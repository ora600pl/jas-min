"""Build the self-contained, bilingual, offline entrypoint. No dependencies."""
from pathlib import Path
import re

BASE = Path(__file__).resolve().parent
SRC = BASE / 'src'


def bundle():
    html = (SRC / 'index.html').read_text(encoding='utf-8')
    html = html.replace('<link rel="stylesheet" href="course.css">',
                        '<style>' + (SRC / 'course.css').read_text(encoding='utf-8') + '</style>')
    for name in ('math.js', 'data.js', 'lessons.js', 'course.js'):
        script = (SRC / name).read_text(encoding='utf-8')
        script = re.sub(r'</script', lambda _: r'<\/script', script, flags=re.IGNORECASE)
        tag = f'<script src="{name}"></script>'
        if html.count(tag) != 1:
            raise RuntimeError(f'Expected exactly one script tag for {name}')
        html = html.replace(tag, '<script>\n' + script + '\n</script>')
    return html.replace('href="../journey.html"', 'href="journey.html"')


if __name__ == '__main__':
    destination = BASE / 'index.html'
    destination.write_text(bundle(), encoding='utf-8')
    print('Built offline PL/EN course:', destination.stat().st_size, 'bytes')
