#!/usr/bin/env python3
"""Check reader links from their delivery location. Requires Python 3.9+."""
import argparse
from collections import Counter
from dataclasses import dataclass
from functools import lru_cache
from html import unescape
from html.parser import HTMLParser
import json
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit


@dataclass(frozen=True)
class Reference:
    url: str
    role: str
    line: int


class Document(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.anchors = Counter()
        self.links = []

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        for anchor in {values.get('id'), values.get('name') if tag == 'a' else None} - {None, ''}:
            self.anchors[anchor] += 1
        for attr in ('href', 'src', 'xlink:href', 'poster'):
            url = values.get(attr)
            if url is not None:
                role = 'base' if tag == 'base' else 'link' if tag in ('a', 'area') else 'asset'
                self.links.append(Reference(url, role, self.getpos()[0]))


MD_LINK = re.compile(r'''(?<!\\)\]\(\s*(?:<(?P<angle>[^>\n]+)>|(?P<bare>(?:\\.|[^\s()\\]|\((?:\\.|[^\s()\\])*\))+))''')
MD_DEFINITION = re.compile(r'^ {0,3}\[[^\]\n]+\]:[ \t]*(?:<([^>\n]+)>|(\S+))')
HEADING = re.compile(r'^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$')
FENCE = re.compile(r'^ {0,3}(`{3,}|~{3,})')
CODE_SPAN = re.compile(r'(?<!`)(`+)(?!`)(.+?)(?<!`)\1(?!`)')


def read_markdown(doc, text):
    html_lines = []
    fence = None
    slugs = set()
    for number, line in enumerate(text.splitlines(), 1):
        delimiter = FENCE.match(line)
        if delimiter and fence is None:
            fence = delimiter[1]
            html_lines.append('')
            continue
        if fence:
            if re.fullmatch(r' {0,3}' + fence[0] + '{' + str(len(fence)) + r',}\s*', line):
                fence = None
            html_lines.append('')
            continue
        # Inline code, including SQL that looks like HTML, does not generate anchors or links.
        inline = CODE_SPAN.sub(lambda m: ' ' * len(m[0]), line)
        heading = HEADING.match(line)
        if heading:
            title = re.sub(r'<[^>]+>', '', heading[1]).replace('`', '')
            stem = re.sub(r'[^\w\- ]', '', unescape(title).lower()).replace(' ', '-')
            slug, ordinal = stem, 0
            while slug in slugs:
                ordinal += 1
                slug = f'{stem}-{ordinal}'
            slugs.add(slug)
            doc.anchors[slug] += 1
        for link in MD_LINK.finditer(inline):
            target = link['angle'] or link['bare']
            target = unescape(re.sub(r'\\([\\()])', r'\1', target))
            role = 'asset' if re.search(r'!\[[^\]]*$', inline[:link.start()]) else 'link'
            doc.links.append(Reference(target, role, number))
        definition = MD_DEFINITION.match(inline)
        if definition:
            doc.links.append(Reference(unescape(definition[1] or definition[2]), 'link', number))
        # Angle-delimited Markdown destinations can begin with '/' and otherwise confuse HTMLParser.
        html_lines.append(MD_LINK.sub(' ', inline))
    doc.feed('\n'.join(html_lines))


@lru_cache(maxsize=128)
def load(path):
    doc = Document()
    text = path.read_text(encoding='utf-8-sig')
    if path.suffix.lower() in ('.md', '.markdown'):
        read_markdown(doc, text)
    else:
        doc.feed(text)
    doc.close()
    return doc


def validate(root, readers):
    root = root.resolve(strict=True)
    problems, results = [], {}
    for reader in readers:
        path = (root / reader).resolve()
        counts = Counter()
        label = str(reader)
        def problem(line, url, reason):
            problems.append(dict(file=label, line=line, url=url, reason=reason))
        if not path.is_relative_to(root) or not path.is_file():
            problem(0, label, 'reader_missing_or_outside_package')
            continue
        doc = load(path)
        for ref in doc.links:
            value = ref.url.strip()
            counts['references'] += 1
            if ref.role == 'base':
                problem(ref.line, value, 'base_changes_document_location')
                continue
            try:
                url = urlsplit(value)
            except ValueError:
                problem(ref.line, value, 'invalid_url')
                continue
            if url.scheme == 'data' and ref.role == 'asset':
                counts['embedded_assets'] += 1
                continue
            if url.scheme in ('http', 'https', 'mailto') or url.netloc:
                counts['external_' + ref.role + 's'] += 1
                if ref.role == 'asset' or (url.netloc and not url.scheme):
                    problem(ref.line, value, 'external_dependency')
                continue
            # Upstream chart indexes also use inert javascript:void(0) map controls.
            if ref.role == 'link' and value.rstrip(';').lower() == 'javascript:void(0)':
                counts['inert_controls'] += 1
                continue
            decoded = unquote(url.path)
            if url.scheme or decoded.startswith(('/', '\\')) or '\\' in decoded:
                problem(ref.line, value, 'nonportable_path_or_scheme')
                continue
            target = (path.parent / decoded).resolve() if decoded else path
            if not target.is_relative_to(root):
                problem(ref.line, value, 'outside_package')
                continue
            if target.is_dir():
                target = target / 'index.html'
            if not target.is_file():
                problem(ref.line, value, 'missing_file')
                continue
            counts['local_references'] += 1
            if url.fragment and target.suffix.lower() in ('.html', '.htm', '.svg', '.md', '.markdown'):
                anchor = unquote(url.fragment)
                hits = load(target).anchors[anchor]
                if hits != 1:
                    problem(ref.line, value, 'ambiguous_fragment' if hits else 'missing_fragment')
                else:
                    counts['checked_fragments'] += 1
        results[label] = dict(counts)
    return dict(files=results, problems=problems)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path, help='actual package directory on disk')
    parser.add_argument('--json', type=Path, help='write detailed results to this path')
    parser.add_argument('documents', nargs='+', help='reader HTML or Markdown paths relative to --root')
    args = parser.parse_args()
    result = validate(args.root, args.documents)
    report = json.dumps(result, indent=2, ensure_ascii=False)
    if args.json:
        args.json.write_text(report + '\n', encoding='utf-8')
    else:
        print(report)
    return 1 if result['problems'] else 0


if __name__ == '__main__':
    raise SystemExit(main())
