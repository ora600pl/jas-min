# Local TOON performance patch

Source: `toon` 0.1.2 from crates.io, authored by Jad Jabbour, declared MIT license.
Upstream repository: https://github.com/JadJabbour/toon-rs

This is the exact version previously resolved for JAS-MIN. The only functional
change is in `src/primitives.rs`: the two unchanged regex patterns are compiled
once with `std::sync::LazyLock`, instead of once for every key/string. Encoding,
number formatting, quoting and normalization retain upstream behavior.

`toon_regression_tests` compares output byte for byte with the registry version,
including nested objects, arrays, Unicode, numeric-looking strings and escapes.
The registry dependency is test-only. Release builds use this local copy.

Keep this patch until an upstream replacement passes the compatibility tests and
the large DNV export replay. Do not silently change TOON format while updating it.
