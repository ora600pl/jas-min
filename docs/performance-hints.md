# Performance hints

The **HINTS** menu uses existing AWR/STATSPACK and plan attachments. Rule version
`2026-09-14.2` separates measured work growth, time support, instance context and
unresolved physical causes. It requires neither new database queries nor an AI
service. HTML, classic AI, the local agent and MCP share the same typed result.

## Signals and conclusions

- `scan_work_inflation`: increased blocks/(short + long table scans).
- `sql_logical_read_work_inflation`: increased SQL gets/execution.
- `row_continuation_work_inflation`: increased continued/ROWID workload ratio,
  evaluated independently from scans. This is instance context, not a percentage
  of chained rows and not attribution to a SQL or segment.

A material work increase can be reported with missing or non-increasing time.
`recurrent_work_inflation` / `provisional_work_inflation` identify such signals.
`recurrent_hypothesis` / `provisional_hypothesis` additionally have observed time
support in the representative comparison; a short time reference still lowers
confidence. `confidence` describes evidence strength (`low` / `moderate`), never
probability of physical fragmentation.

Physical `mechanisms` remain empty without confirming evidence. Alternatives
explicitly include sparse table/index blocks, migration, more rows, wider rows,
**necessary row continuation due to wider data**, changed binds, useful work,
plans, caching and CR/undo. Neither index access nor a high counter alone proves
or excludes a physical cause. No automatic MOVE, SHRINK or REBUILD is recommended.
A JAS-only analysis can finish with measured work growth and an unresolved cause;
additional space/row measurements are optional external confirmation.

`persistent_cost_observation` is an absolute footprint, with no baseline
comparison or growth/stability claim. Its `confidence` is `not_applicable`.

## Independent observed histories

Work observations survive even when **both** CPU and elapsed are missing. Each
time domain separately retains valid observations, including those in windows
without TOP gets. Each domain uses its own numerator, executions and observed
reference intervals. Missing TOP, disabled collection, nonfinite values and
unfinished executions (`executions=0`) are unknown, never measured zeros.

For collections spanning at least 24 hours, profiles match weekday/weekend,
start hour and exposure class (<5m, <30m, <2h, longer). Short captures use local
history. Instance execution-normalized and wall-time-normalized measurements
use separate profiles. Selection uses only earlier observations and does not
classify profiles by the cost being tested. Calendar matching does not establish
equal binds, shifts, holidays or useful work.

The work reference considers up to 10 recent comparable observations and the
first 3 comparable work observations. After a work-growth hit with sufficient
work history, that reference freezes **without waiting for time support**.
Time references use their own last 10 observed values in the same profile,
ending no later than the work reference. This preserves a pre-growth reference
rather than accepting a later entry into TOP time as normal. An explicit baseline
range includes all valid domain observations within its bounds. Baselines are
not assumed healthy; their periods and coverage are exposed separately.

Work recurrence needs two hits in three observed comparable opportunities, with
at least two recurring comparisons that have sufficient work references. Missing
work is counted but not inserted as a negative hit. At least 3 baseline work
observations and 120 seconds are required for sufficient work history. Time
reference sufficiency is assessed separately. Repeated windows show persistence,
not independent experiments or calibrated statistical significance.

## Thresholds and workload safeguards

For a positive baseline, growth must exceed the percentage floor and a robust
noise floor: `max(log(1 + growth), noise_multiplier * 1.4826 * MAD(log(history)))`.
References are medians of per-window ratios. Defaults are 25% work growth, 15%
time growth, 1 extra get/block per operation and 0.1 ms extra time per execution.
Continuation uses an absolute difference of 0.01 continued encounters/ROWID fetch.
A zero baseline uses absolute differences without an infinite growth percentage.
These are screening heuristics, not calibrated false-positive rates.

Scan eligibility requires 30 scan starts. Direct/PX/cache/IM activity does not
exclude the whole window. Each path descriptor is checked separately; counts
are not subtracted or assumed disjoint. A descriptor change above 0.15, >2x change
in scans/execution, or materially changed observed SQL execution mix suppresses
only the aggregate scan-ratio inference.

Continuation eligibility requires 30 ROWID fetches. It independently checks
ROWID/execution changes (>2x) and observed SQL execution mix. Missing mix evidence
is disclosed. A changing or unobserved denominator is not fabricated as zero.
Neither instance check suppresses an otherwise measurable SQL work signal.

Time Model DB CPU/DB Time seconds are preferred over rounded Load Profile rates.
Instance time divided by global execution count is **not scan or continuation
CPU**. HTML names its scope and displays ms/execution. When executions are absent,
a separately grouped wall-time rate is used. Instance library/transaction waits
above 50% of DB Time prevent elapsed from supporting an aggregate access-time
inference; a SQL elapsed comparison remains available with a wait confounder and
low confidence. Work growth remains visible independently.

Invalid/overlapping intervals are excluded locally and split epochs. Unprovided
startup/container changes cannot be reconstructed. Missing plan hashes and one
supplied plan are not evidence of identical runtime branches or useful work.

## Segment selection and contextual controls

Instance scan candidates require material logical-read rate growth (25% and at
least 100 reads/s by default) plus at least 100 excess reads/s beyond observed
activity growth. Activity uses the larger available growth factor of scan starts/s
and global executions/s, over the segment's own observed windows. This is a
conservative exclusion screen, **not a segment execution denominator**. If no
activity reference is available, the segment stays in context.

`segment_context` retains examples with small growth, growth explained by activity,
or unavailable activity. `sql_controls` shows contemporaneous SQLs without
material gets/execution growth and their available CPU comparisons; it does not
invent a SQL-to-segment mapping or identify a laboratory control by name.

For SQL work-growth hints, supplied plan names can retain TABLE/INDEX candidates
even when segment reads/s are flat or falling: SQL unit work can increase while
volume falls. The plan parser is shared with the existing API and uses observed
hashes. Names identify candidates, not runtime cost attribution. Owner/container
ambiguity is explicit; OBJ#, DATAOBJ#, owner, PDB and subobject remain identity
components. Duplicates and changed physical epochs are not combined. Without a
usable plan, simultaneous TOP segments are not presented as the SQL's objects.
Continuation-only instance hints do not assign affected segments.

## Report presentation

Cards show **Observed evidence**, its scope, the observed episode, a representative
comparison and the state in the latest supplied window. A work-history chart and
expandable table show observed values and references. Missing intervals and different
profiles are not joined. Later lower work does not imply a repair; missing or
non-comparable latest data is explicitly unknown. This describes the supplied
capture, not the current live production state.

`context_evidence` keeps global continued row outside SQL/scan evidence, with
**Instance context — not SQL attribution** on SQL cards. A non-increase provides
no positive continuation support in that comparison, without proving absence in
a specific table. Card-level limitations describe the representative comparison;
earlier short baselines retain limitations on their own comparison/table row.

The first five work-growth cards are expanded. Further cards and details are
collapsible. Missing plans produce a short message; missing segments produce
`No conclusive data to identify affected segments.` once per card.

Absolute cost observations appear last in a separate collapsed section, without
`Prove`, before/after arrows or affected-segment claims. They require at least 3
common gets/CPU observations, 1 million gets, 60 CPU seconds and 1% of collected
DB CPU when available, plus an observed table/index row source. These thresholds
prioritize measured footprint; they do not establish inefficiency.

## Shared contract and compatibility

`ReportForAI.performance_hints` is the complete shared result. MCP/local details
use `get_precomputed_analysis(section="performance_hints")`, with optional
`rule_id`, `hint_id`, `scope`, `offset`, `limit`. Scope `instance` selects scan work;
`instance:row_continuation` selects continuation; `sql:<SQL_ID>` selects SQL work.
Exact scopes also narrow profile coverage; global rule totals remain global.
MCP additionally requires its session/project identifiers.

Key fields:

- `signal_kind`, `assessment_status`, `time_impact_status`, `episode_status` and
  `trajectory` describe the symptom, time support and temporal state.
- `comparisons[].cost` and `cost_threshold_pct` are nullable. `cost_evaluations`
  includes both domains, their own reference periods, observed counts, comparison
  or explicit absence, status and reference sufficiency. `baseline_sufficient`
  on the parent comparison describes **work** history.
- `context_evidence`, `segment_context` and `sql_controls` are not physical-mechanism
  proof. `alternative_explanations` separates hypotheses from measurements.
- Absolute observations have null baseline/comparison and empty comparison
  evidence, mechanisms and affected-segment candidates. `observed_metrics` and
  `observed_segments` expose value, total, own exposure/unit, period and source.

For compatibility `hints` / `hints_total` include both growth and absolute records;
status distinguishes them. `hypotheses_total` counts non-absolute records, including
work-only signals; `cost_observations_total` counts absolute records. The compact
preview includes signal, time and episode status. Older reports deserialize with
empty new fields and old non-null cost objects; regenerate to obtain new analysis.
Absent reports return `not_computed`. The stable rule ID remains
`possible_table_fragmentation`.

`observed_cost_seconds` orders records within a category: baseline-relative time
excess for supported comparisons, or absolute footprint for observations. Zero
for a work-only hint is **not a measured zero time impact**; consult time status.
This ranking measure is not recoverable time and must not be summed across cards.

## Policy and validation

`--hints-policy policy.json` accepts omitted defaults, rejects unknown fields and
validates finite thresholds before analysis. New fields are
`minimum_rowid_fetches` (30), `minimum_continuation_delta_per_rowid` (0.01) and
`minimum_segment_excess_reads_per_second` (100). Existing work/time thresholds and
calendar/profile controls retain their defaults. Legacy `workload_band_pct`,
`maximum_baseline_ratio_spread`, `minimum_extra_blocks_per_second` and
`minimum_db_cost_delta_s_per_second` remain readable but are not v1 exclusion gates.
`minimum_top_coverage` (0.8) describes work coverage and affects confidence only.

Tests exercise missing both time domains, time without gets, frozen pre-growth
references, explicit independent exposure, known non-increasing time, mixed
workloads, independent continuation, global-counter scope, activity-only segment
growth, later lower/missing work and period-specific limitations. Recorded tests
include the 134 blocks/scan nested-loop case, live/deleted indistinguishable
counters and HEAD/full projection. Shared classic/local/registered MCP tests cover
both work-only and time-supported signals and absolute observations.

Optional read-only replays use `JASMIN_HINT_REPLAY_INPUT`,
`JASMIN_HINT_REPLAY_OUTPUT`, `JASMIN_HINT_REPLAY_STEM` for szpital, and
`JASMIN_HINT_REVIEW_INPUT`, `JASMIN_HINT_REVIEW_OUTPUT` for the reviewed EmptyCalories
hourly capture. Production data is not checked into the fixtures. The szpital
capture has no weekend or labelled fragmentation ground truth: accuracy and
weekend thresholds remain uncalibrated.
