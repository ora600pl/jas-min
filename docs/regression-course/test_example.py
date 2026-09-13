"""Test the privacy allowlist and deterministic offline bundle without private inputs."""
import json
import unittest
from pathlib import Path
from prepare_example import extract
from build_course import bundle

BASE = Path(__file__).resolve().parent


class ExampleTests(unittest.TestCase):
    def setUp(self):
        text = (BASE / 'src/data.js').read_text(encoding='utf-8')
        self.data = json.loads(text.removeprefix('window.RECORDED_DATA=').strip().removesuffix(';'))

    def test_no_arbitrary_source_strings(self):
        def walk(value, path=()):
            if isinstance(value, dict):
                for key, item in value.items():
                    walk(item, path + (key,))
            elif isinstance(value, list):
                for item in value:
                    walk(item, path)
            elif isinstance(value, str):
                self.assertEqual(path, ('meta', 'features'))
        walk(self.data)
        self.assertEqual(set(self.data), {'y', 'x', 'dy', 'dx', 'z', 'xmean', 'xstd',
                                         'ymean', 'ystd', 'focus', 'focusRow', 'meta', 'fits'})
        self.assertEqual(set(self.data['meta']), {'n', 'snapshots', 'missing_counts',
                         'ridge_lambda', 'en_alpha', 'en_lambda', 'en_folds',
                         'en_cv_ratios', 'en_cv_means', 'en_cv_se', 'en_cv_selected',
                         'en_cv_best', 'huber_delta', 'q95_lambda', 'q95_tau',
                         'features', 'q95_converged'})

    def test_allowlist_ignores_identifiers_even_inside_traces(self):
        private = dict(self.data)
        private['meta'] = dict(private['meta'], instance='PRIVATE_SENTINEL',
                              source='/private/sentinel', checks={
                                  'q95_converged': self.data['meta']['q95_converged']})
        private['traces'] = {key: [dict(value, note='PRIVATE_SENTINEL')]
                             for key, value in self.data['fits'].items()}
        private['times'] = ['PRIVATE_SENTINEL']
        private['ids'] = ['PRIVATE_SENTINEL']
        self.assertEqual(extract(private), self.data)
        self.assertNotIn('PRIVATE_SENTINEL', json.dumps(extract(private)))
        private['y'] = ['not a number']
        with self.assertRaises(ValueError):
            extract(private)

    def test_generated_file_matches_sources(self):
        self.assertEqual(bundle(), (BASE / 'index.html').read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
