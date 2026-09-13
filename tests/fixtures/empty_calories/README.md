# Recorded Oracle lab inputs

Copied from oracle-EmptyCalories/data without semantic changes; whitespace compacted. These are recorded SKIPPER windows, not AWR snapshots. No new database work is performed by the tests. Missing SQL/segment/host CPU observations must stay unavailable.

- `scan_degradation.json`: source SHA-256 `d9d3409115336dee571a8956f8fc5bbbb43bf553a9b6d498b0739f4a72ca63b6`.
- `migr_degradation.json`: source SHA-256 `46f6168c1ee04468169ab9c36b72b40e02b4bfd67ce0ef1681cef2325d871160`.

- `native_scan_targets.json`: projection of the 24 native AWR windows (SNAP 192–216)
  from `oracle-EmptyCalories/jasmin_only/native_scan_prefix.json`, source SHA-256
  `8e437783c97bbcb341cea23d5a3e944db35f12e5b9768156db7e614acc97d1b1`.
  Retains snapshot identity, original Load Profile, Time Model, all instance counters
  and availability masks without numeric changes. Other domains are omitted; the
  test adapter leaves them empty. This is the recorded short-window precision case,
  not the SKIPPER fixtures above. With Time Model targets, the scan-blocks active ranks
  are Ridge 2, Huber 2 and Q95 5 under the default model settings.
