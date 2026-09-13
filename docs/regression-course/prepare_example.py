"""Extract an identifier-free, numeric teaching payload from a local trace.

Usage: python3 prepare_example.py /path/to/private/trace.json
Only the explicit numeric allowlist below is exported. No input path is stored.
Removing identifiers does not guarantee that measurements cannot be linked.
"""
import json
import math
import sys
from pathlib import Path

BASE = Path(__file__).resolve().parent


def numbers(value):
    if isinstance(value, list):
        return [numbers(item) for item in value]
    if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value):
        return value
    raise ValueError('Expected finite numeric data only')


def extract(source):
    out = {key: numbers(source[key]) for key in (
        'y', 'x', 'dy', 'dx', 'z', 'xmean', 'xstd', 'ymean', 'ystd', 'focus', 'focusRow')}
    meta = source['meta']
    out['meta'] = {key: numbers(meta[key]) for key in (
        'n', 'snapshots', 'missing_counts', 'ridge_lambda', 'en_alpha', 'en_lambda',
        'en_folds', 'en_cv_ratios', 'en_cv_means', 'en_cv_se', 'en_cv_selected',
        'en_cv_best', 'huber_delta', 'q95_lambda', 'q95_tau')}
    # Fixed generic metric names, never copied from arbitrary source strings.
    out['meta']['features'] = ['PX Deq: Execution Msg', 'cursor: pin S wait on X',
                               'library cache: mutex X', 'resmgr:cpu quantum']
    if not isinstance(meta['checks']['q95_converged'], bool):
        raise ValueError('Expected a boolean convergence flag')
    out['meta']['q95_converged'] = meta['checks']['q95_converged']
    out['fits'] = {model: {key: numbers(source['traces'][model][-1][key])
                          for key in ('beta', 'intercept')}
                   for model in ('ridge', 'elastic', 'huber', 'quantile')}
    n = out['meta']['n']
    if len(out['z']) != n or len(out['dy']) != n or any(len(row) != 4 for row in out['z']):
        raise ValueError('Expected a four-input trace with aligned rows')
    return out


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('Usage: python3 prepare_example.py /path/to/private/trace.json')
    payload = extract(json.loads(Path(sys.argv[1]).read_text(encoding='utf-8')))
    target = BASE / 'src' / 'data.js'
    target.write_text('window.RECORDED_DATA=' + json.dumps(payload, separators=(',', ':'), allow_nan=False) + ';\n', encoding='utf-8')
    print('Prepared numeric teaching example:', payload['meta']['n'], 'rows')
