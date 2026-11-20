import importlib.util
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest


HERE = Path(__file__).resolve().parent
SCRIPT = HERE / 'validate-report-package.py'
spec = importlib.util.spec_from_file_location('report_package', SCRIPT)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class PackageLinks(unittest.TestCase):
    def test_files_fragments_and_package_boundaries(self):
        with TemporaryDirectory() as scratch:
            root = Path(scratch) / 'portable'
            root.mkdir()
            (root / 'units').mkdir()
            (root / 'units' / 'source a.html').write_text('<h2 id="delta">Trend</h2>')
            (root / 'index.html').write_text('''<a href="units/source%20a.html?day=2#delta">Proof</a>
<a href="units/missing.html#delta">Missing</a><a href="units/source%20a.html#typo">Typo</a>
<a href="file:///tmp/author.html">Bad export</a><a href="../outside">Escapes</a>''')
            result = module.validate(root, ['index.html'])
            self.assertEqual(result['files']['index.html']['checked_fragments'], 1)
            self.assertCountEqual([p['reason'] for p in result['problems']],
                                  ['missing_file', 'missing_fragment', 'outside_package', 'nonportable_path_or_scheme'])

    def test_markdown_and_html_share_anchor_integrity(self):
        with TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / 'evidence (1).html').write_text('<p id="a">Measured</p>')
            (root / 'index.md').write_text('''## Observed window

[Source](<evidence (1).html#a>) · [Back](#observed-window)
<a id="cost"></a> [Time](#cost) · <a href="missing.md">Miss</a>
```sql
select '[A](absent.html)', '<a href="example.html">' from example;
```
`[local](ignored.txt)` and [baselines][fixed]
[fixed]: <evidence (1).html#a>
''')
            result = module.validate(root, ['index.md'])
            self.assertEqual(result['files']['index.md']['checked_fragments'], 4)
            self.assertEqual([p['url'] for p in result['problems']], ['missing.md'])

    def test_missing_and_ambiguous_destinations_are_separate(self):
        with TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / 'index.html').write_text('<a id="r"></a><a name="r"></a><a href="#r">Repeat</a>')
            result = module.validate(root, ['index.html', 'missing.html'])
            self.assertCountEqual([p['reason'] for p in result['problems']],
                                  ['ambiguous_fragment', 'reader_missing_or_outside_package'])


if __name__ == '__main__':
    unittest.main()
