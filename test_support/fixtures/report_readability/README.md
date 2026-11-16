# Report readability acceptance examples

`cases.json` contains two neutral, deliberately incomplete report examples:
a remote query and a CPU investigation. They exercise the distinction between
reader prose, technical measurements, and a sequence of conditional actions.
The examples are not measurements of a running service or a complete audit.

The fixtures retain exact plan counts, operation numbers, percentages, missing
measurements, and the distinction between observations and hypotheses. The
renderer tests check that this evidence survives, that the visible explanation
precedes technical detail, and that the CPU investigation has one initial
comparison followed by two separate conditional investigations.

Run the ordinary tests with:

```sh
cargo test report_readability_examples
```

To create a fresh Markdown and HTML preview through the production renderer,
set `JASMIN_READABILITY_PREVIEW` to a new absolute HTML path and run:

```sh
cargo test preview_report_readability_examples -- --ignored
```

The output directory must already exist. The test refuses to replace either
output file. This is a renderer preview; it bypasses the complete-report
publication gate because the remaining report categories are intentionally
absent. No model or database connection is required.

Automated assertions preserve structure and evidence. They do not prove that
arbitrary generated prose is clear, or that the hypotheses describe a real
system. Visual and editorial review remain necessary.
