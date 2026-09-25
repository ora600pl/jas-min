# Validation

## 2026-09-25 — standalone model lessons and chapter 04 Signal

- Fifteen actual read-only Claude Opus 5.5 consultations: three independent sessions per model, two for Signal and one final adjudication. No private conversation history was supplied. Model telemetry confirmed `claude-opus-5-5`. The final verdict was **GO**, with prior mathematical/pedagogical blockers resolved and no new mathematical errors found. The reviewer did not execute the UI; scope and decisions are recorded in [REVIEW.md](REVIEW.md).
- `node docs/one-more-query/package.cjs`: **51 numerical/static/content/source-contract checks passed**. ZIP integrity, exact four-entry allowlist and byte-for-byte HTML identity passed. Source contracts use the current Rust module paths. Original observations, retained model coefficients and the fitting engine were not modified.
- Final `browser-test.cjs`: **399 checks passed**, no uncaught JavaScript errors, localhost-only network requests, direct `file://` launch passed. Both languages, all five chapters at 320/390/900/1440 px, all four dialogs at 320/390/1440 px, and all 24 language/model/percentile combinations in Signal were exercised. Tests cover the fixed-data EN controls, expanded EN arithmetic, Huber threshold, 100-cell Q95 lab, accessible model explanations from Signal, six provenance steps, Gaussian elimination, exports and reduced motion.
- Visually inspected the 320 px Elastic Net arithmetic table, desktop Q95's numbered grid and expanded Signal provenance. Browser checks assert no horizontal overflow in the tested dialog/provenance layouts. Native iOS Safari was not tested.
- New numerical checks include EN's full arithmetic and complete-square identity, the Q95 100-observation objective and flat minimum, a Q95-below-mean counterexample, Huber's illustrative loss, Signal's zero-filled-entry bounds, matrix diagonal and other-coefficient contributions. Input data and retained fits remain identical; only Ridge is refitted interactively.
- HTML: **254,372 bytes**, SHA-256 `9fca2f34136da90bfd29a0902eed2f8d5eb9f52d392899324ba3d40cffe89230`. The regenerated ZIP and its checksum are recorded in `dist/manifest.json`; ZIP timestamps may change its checksum without changing the HTML.
- The course performs no Oracle, LLM or MCP calls and sends no observations off-device. Opus review was a separate authoring activity. No missing row mask or window duration was reconstructed. No learner study, fresh out-of-sample model benchmark or company-hosting upload was performed.

Entries below are historical results for earlier builds, not additional checks on the latest build.

## 2026-09-25 — fixed observations and parameter provenance

- Two additional actual read-only Claude Opus 5.5 consultations, confirmed by `modelUsage`. The second made publication conditional on correcting the synchronized-lab intro, aligning Ridge PL/EN copy, and verifying three implementation details. Those conditions were completed. The reviewer did not run tests; [REVIEW.md](REVIEW.md) records the critique and decisions.
- `node docs/one-more-query/package.cjs`: **46 numerical/static/content/source-contract checks passed**. ZIP integrity, exact four-entry allowlist and byte-for-byte HTML identity passed. Source-contract checks cover CLI defaults, automatic EN configuration, soft thresholding, final target unscaling, Huber's delta floor and Q95 regularization. No Rust algorithm or retained observation/coefficient was changed.
- Final `browser-test.cjs` run: **275 checks passed**, no JavaScript exceptions, localhost-only requests, direct `file://` launch passed. Both languages, all chapters at 320/390/900/1440 px and model dialogs at 320/390/1440 px were tested. Added assertions keep the EN observation fixed at four lambda values, synchronize both EN controls in both directions, keep all five Huber observations fixed at three thresholds, expose per-model parameter provenance and check expanded EN mathematics/settings at phone width.
- Visually inspected phone-width Elastic Net and desktop Huber: fixed-measurement labels, predictions, zoomed EN contribution/error bars and the Huber threshold control. Native iOS Safari was not tested.
- Numerical tests verify the fixed-data EN optimum, exact positive zero at lambda >= 3, candidate-card deduplication, Huber optima over varying thresholds, the actual 4.240123 residual within its slider range, median/MAD example values and real-fit invariance. PL/EN checks reject the removed database-cost/resource-consumption rebuttals.
- HTML: **224,780 bytes**, SHA-256 `f1eacbd9f59bdb06f393c1df3fa1cbc0af522b27b45631c13ddf497048dc932c`. ZIP: **84,306 bytes**, SHA-256 `7340b81efa7a27b24fc7eebe98de863e844a7f9bd2d7db79311503923d29f0ac`. The generated manifest identifies the packaged files.
- The course explains current JAS-MIN automatic EN lambda selection; it does not rerun that selection or invent missing candidate scores. Ridge lambda 0.05 is configured, not automatically selected. The company-hosting upload remains separate; no Oracle, LLM or MCP calls were added to the course. Teaching effectiveness has not been tested with learners.

## 2026-09-25 — problem-first model explanations

- Three actual read-only Claude Opus 5.5 review rounds, with `modelUsage` confirming `claude-opus-5-5` each time. The third accepted the teaching structure and made publication conditional on normalizing Elastic Net's negative zero and passing browser checks. Both conditions were completed. The reviewer did not run the UI; decisions are in [REVIEW.md](REVIEW.md).
- `node docs/one-more-query/package.cjs`: **42 numerical/static/content checks passed**, ZIP integrity and exact allowlist passed, zipped HTML matched the built HTML. Original observations, four-feature fits, source caveats, current-Ridge handoff and Q95 exclusion remain covered by regression tests.
- `browser-test.cjs`, using installed Chrome with a separate test profile: **237 checks passed**, no JavaScript exceptions, no external requests, direct `file://` launch passed. Tested both languages, all five chapters at 320/390/900/1440 px; model dialogs at 320/390/1440 px; keyboard focus return; primary and advanced sliders; the L1 toggle; Q95 frequencies and fractional means; actual-data invariance; Gaussian elimination; exports and reduced motion. Native iOS Safari was not tested.
- Visually inspected desktop and phone-width captures of the primary Ridge, Elastic Net and Huber experiments. Signed bars now have a zero reference; the small Elastic Net difference has its own scale. Mathematics remains collapsed until requested, and separate exercises disclose their independent settings.
- New numerical checks cover Ridge candidate sensitivity, the exact L1 zero interval and positive-zero formatting, the Huber constant-fit optimum over a range of jump sizes, its raw-MAD floor, two Q95 frequency optima, the real Huber preset within its slider range, and text branches on both sides of the Huber threshold.
- Final HTML: **212,588 bytes**, SHA-256 `36fcef0d22cb219bee5b3320afd6829a0d2f1a64135317e3eb762139e99a0160`. Upload ZIP: 80,117 bytes, SHA-256 `adfd6929d14bb2de13b063c02669a3a39aba6070e3a7959cf4a624dfdec6a039`. The generated manifest holds these checksums.
- No model-selection validation was run. Lambda 0.05 is explicitly an initial setting, not a demonstrated best value. Illustrations are not new Oracle observations. Teaching effectiveness has not yet been tested with learners. The company website was not modified; the refreshed ZIP is for a separate upload.

## Historical validation — 2026-09-24

## Publication build — standalone upload and GitHub Pages

- Rebuilt and passed all **36 numerical/static checks after removing the legacy course** from active local resources. Build no longer imports any retired course or raw report. Older local course/animation exports were moved to recoverable Trash; underlying performance reports were not modified. The previous tracked course was already absent from remote main (retired in 9924949); Git history was not rewritten.
- `package.cjs` builds/tests before packaging, checks ZIP integrity, checks the exact four-entry allowlist, and compares the zipped HTML byte-for-byte with the tested build. Pages output is limited to `index.html` and `.nojekyll`; unexpected files in that directory abort packaging.
- Upload ZIP entries: `jas-min/index.html`, `jas-min/.htaccess`, `jas-min/.nojekyll`, `UPLOAD.md`. HTML: 187,810 bytes, SHA-256 `f40dd7da4ca963701a2a24ccf66b779c0fd3af1b03a883c5fe8acdcf756398cd`. Per-build archive checksum is recorded in the generated `dist/manifest.json`.
- GitHub Actions pins official action commits, builds under Node 24, runs the numerical suite and uploads only the explicit Pages directory. No custom domain or DNS change is involved.
- The company-hosting destination returned HTTP 404 during preparation. No upload to that hosting account was performed; Apache-specific directives have not been executed on that server. The included instructions describe the subdirectory-only configuration and a fallback for hosts disallowing it.

## Review edition 1 — whole-project critique and revised handoff

- Two actual Opus 5.5 rounds; model telemetry confirmed claude-opus-5-5. Sessions: 44e004b3-d735-49e2-a209-588bdb085fc3 and 759efaf1-b409-45e3-9f69-73989237b71a. The second round reviewed revised source and explicit counterarguments. Opus did not run the UI. Decisions and rejected overclaims are recorded in REVIEW.md.
- Numerical/static suite: **36 checks passed**. New tests cover every Gaussian row operation and back substitution, pivot recording, current-lambda handoff consistency, a deliberately different leader, no-positive-lead handling, Huber's retained penalized weighted equations, explicit EN alpha convention, warning-bearing raw export, baseline slider detent, both-language story panels, the actual MAX quiz, explicit MCP session/project placeholders, and generated-build/source identity.
- In-app browser: **40 layout cases** (five chapters × PL/EN × 320/390/733/1440px), all with headings present and no document-level horizontal overflow. Initial guess preserved focus and selection, survived a language change, and returned with its actual ranks in chapter four. Brand navigation retained English.
- All six valves checked. Reduced-motion flow has a visible static explanation. Polish counts read “4 iteracje”, “22 iteracje”, “1 brak” and “963 braki”.
- All eight PL/EN model dialogs and their expanded labs checked at 320px: initially collapsed maths, updating slider readouts, no invalid numeric text, 279px content/scroll widths. All **24 model/metric/language rankings** retained six provenance steps, explicit Q95 exclusion and no document overflow.
- All ten Gaussian operations checked through the live slider in Polish. Numerical rendering of every step in both languages is covered by the static suite. Matrix and arithmetic visually inspected on a 320px screen; AAS primer inspected at 390px.
- Setting live Ridge λ to 10 updated the final fact, receipt and proposed request consistently. Source export contents are validated by unit tests. An in-app-browser download-event wait timed out without a console error; therefore this turn does **not** claim verified file delivery via that browser. The exact evidence JSON is also inspectable in the page. Earlier standalone-browser download results below are historical, not a fresh execution of that runner.
- Source observations and retained fitted coefficients were unchanged. No live Oracle/LLM/MCP operation, data repair, public deployment, commit or push.
- Final evidence JSON preview was parsed directly from the visible page: current λ = 0.05, P99, matching leader/request, explicit session placeholders and Q95 excluded. English final back substitution was also checked. No captured browser warnings/errors. Final LAN preview returned HTTP 200 with 187,807 bytes on port 8769; the existing localhost preview remains on 8768. Viewport override was reset and the Polish opening retained for review.

## Follow-up: nine worked-maths comments and Opus review

- Consulted Opus 5.5, confirmed by modelUsage: claude-opus-5-5. Numerical/pedagogical review used a scoped anonymous brief with no source identifiers or raw reports. Adopted corrections and evidence-limited rejections are recorded in README.md.
- Numerical/static suite: **27 checks passed**. Added exact minima for illustrative L2/L1 objectives; Huber MAD provenance, continuity, slope, weights and real residual; Q95 frequency-dependent optima and flat interval; percentile convention and actual interpolation operands; dynamic SD and joint-equation verification; both languages and all model/metric/lambda provenance renderings.
- Live in-app browser: all eight language/model panels open, keep maths initially collapsed, expand and update with slider changes. Closing works. Native Escape visibly restored Ridge focus; a locator-specific focus assertion timed out, so it is not counted as an automated assertion.
- Q95 UI checked at predictions 10 and 30 for all three frequencies: totals 19/4, 19/19 and 19/20 respectively.
- Live ranking checked for **24 combinations**: PL/EN × four models × P90/P99/MAX. All six provenance steps remain present with finite rendered values. The provenance button opens the actual Gaussian system.
- At 390px, document width equals scroll width. All four expanded laboratories fit their dialog body without horizontal overflow (349px client/scroll width). Visual inspection included the percentile guide, Ridge comparison and Huber worked loss.
- Final-build check at 320px: all four EN labs fit the 279px dialog body, including expanded Ridge minimum derivation. The ranking has no horizontal document overflow and its percentile explanation stays expanded when switching P99 to P90.
- Existing reusable browser-test.cjs was extended with lab and provenance assertions. This turn used the in-app browser for UI verification; the historical full-suite result below is not a claim that the updated standalone runner was executed.
- Original observations, numerical fitting methods and retained coefficients were not changed. New illustrative arithmetic helpers are isolated from the real ranking and evidence packet.

## Follow-up: human-first beer explanations

After the four annotated browser comments, model cards were rewritten without unexplained mathematical terms and made clickable. Added PL/EN beer-shaped dialogs with optional mathematical detail.

- Numerical/static suite rerun: **18 checks passed**, including all eight explanations, the recomputed real Huber example, EN/Q95 caveats, standalone build and outbound-network restrictions.
- All four dialogs in both languages opened in the in-app browser; verified plain text, initially collapsed maths, expansion and close controls.
- Native Escape closes the dialog and restores focus to the invoking card. Ridge dialog visually inspected in the annotated 733 × 861 viewport.
- The reusable browser test was extended to cover opening, detail expansion and focus restoration for all eight language/model combinations. The 112-check result below records the earlier full-prototype run, not a claim that the extended suite was rerun in this follow-up.
- Model fitting and input data were not changed.

## Numerical and static checks: 15 passed

Command: node docs/one-more-query/test.cjs

- Matrix dimensions and finite values: 1,339 observations × 5 measurements.
- Adjacent differences, sample standardization and retained statistics agree.
- Locally recomputed Ridge coefficients agree with the existing recorded fit to better than 1e-9.
- Normal equations, positive-penalty variants and Gaussian row operations checked.
- P90/P99 ranking reversal reproduced from actual values.
- Negative coefficients are not turned into positive scores.
- Q95 remains ineligible; missingness and no-execution flags survive export.
- UTF-8 JSON byte counts and illustrative context arithmetic checked.
- Self-contained build syntax and outbound-network-denying CSP checked.
- Approved sample payload checked for source-environment identifiers.

## Browser checks: 112 passed

Playwright with installed Google Chrome, using a separate temporary browser profile.

- All five scenes in PL and EN at widths 320, 390, 900 and 1,440 px.
- Headings present, no document-level horizontal overflow.
- Choice preservation across language changes.
- Scale switching and changing chart geometry with motion enabled.
- Context calculator and both input-strategy choices.
- All six calculation valves, animated flow and reduced-motion control.
- P90/P99 ranking reversal, live lambda changes, Gaussian step control.
- Q95 exclusion visibly present in the selected model view.
- All three MCP decision responses and all three quality-quiz responses.
- Downloaded evidence packet retains quality caveats and Q95 exclusion.
- Data dialog and keyboard dismissal.
- Direct file:// launch works without a local server.
- No uncaught browser exceptions; observed request origin only the localhost preview.

Manual visual review covered desktop charts, desktop distillation and the revised mobile distillation layout. The local preview was also opened and visibly verified in the in-app browser, then retained as a deliverable.

## Not validated / deliberately not implemented

- No live Oracle, LLM or MCP calls; no SQL attribution results fabricated.
- No full-course completion, learner study or claim of learning efficacy.
- No public deployment, commit or GitHub push.
- No recovery of missing row masks or window durations.
- EN/Huber/Q95 are retained numerical fits; only Ridge is recalculated interactively in this prototype.
- No measured model-specific tokenization, pricing comparison or actual full-AWR compression benchmark.

The local preview server serves only this prototype directory on 127.0.0.1:8768. The HTML is also usable after that preview server stops.
