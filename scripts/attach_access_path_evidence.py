#!/usr/bin/env python3
"""Create a new JAS-MIN input by joining explicit SQL/segment evidence to exact windows."""
import argparse
import importlib.util
from pathlib import Path
import shutil
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("collector", ROOT / "jas-min-collector.py")
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    try:
        if args.output.exists():
            raise collector.CollectorError("Output already exists; choose a new file")
        with tempfile.TemporaryDirectory(prefix="jasmin-access-path-") as directory:
            temporary = Path(directory) / "enriched.json"
            shutil.copyfile(args.input, temporary)
            collector.merge_access_path_evidence(temporary, args.evidence)
            # Exclusive creation prevents overwriting an artifact created during validation.
            with args.output.open("xb") as output:
                output.write(temporary.read_bytes())
        print(args.output)
    except (collector.CollectorError, OSError) as exc:
        print(str(exc), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
