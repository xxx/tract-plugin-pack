# Mutation testing

[cargo-mutants](https://mutants.rs/) changes production code and checks whether
the tests catch each change. Run the commands below from the workspace root.

## Supported scopes

| Package | Mutation scope | Configuration |
|---|---|---|
| `tract-dsp` | All library modules and FFT features | [mutants.toml](../../.cargo/mutants.toml) |
| `tract-effects` | All library modules and features | [mutants-tract-effects.toml](../../.cargo/mutants-tract-effects.toml) |

Complete coverage of each crate through reviewed increments before moving to another crate.

## Run locally

Install the tools:

```bash
cargo install cargo-nextest --locked
cargo install cargo-mutants --version 27.1.0 --locked
```

Preview mutations or run a full scope:

```bash
cargo mutants -p tract-dsp --list
cargo mutants -p tract-dsp
cargo mutants -p tract-effects --config .cargo/mutants-tract-effects.toml
```

Use `--file` to focus on one module:

```bash
cargo mutants -p tract-dsp --file tract-dsp/src/boxcar.rs
cargo mutants -p tract-effects --config .cargo/mutants-tract-effects.toml \
  --file tract-effects/src/modulation.rs
```

Each configuration enables all features and uses the Cargo and nextest `mutants`
profiles. Cargo disables debug symbols; nextest runs one test at a time, stops
at the first failure, and disables retries. Add `-j 2` to run two mutation jobs
concurrently. Use `--file` to narrow the scope; `--re` adds alternatives to the
configured expression rather than narrowing it.

## Review results

Results appear in the ignored `mutants.out/` directory:

- `missed.txt`: mutations that survived.
- `timeout.txt`: mutations whose tests exceeded the time limit.
- `unviable.txt`: mutations that could not compile.
- `outcomes.json`: structured results, with per-mutation diffs and logs alongside.

Review survivors against the intended behaviour and strengthen tests where
coverage is missing. A timeout does not prove that an assertion caught a mutation.
Exclude equivalent mutations individually in the relevant configuration, with an
explanation; do not add blanket exclusions to hide survivors.

## CI behaviour

The [Mutation testing workflow](../../.github/workflows/mutation-testing.yml)
runs separate jobs for each registered package. Pull requests use `--in-diff`
against the PR's merge base, selecting only mutations that intersect changed
production code within a registered scope. Manual dispatch runs the full scopes.
Both modes fail on survivors or timeouts and upload per-package report artifacts;
PR artifacts also include `pr.diff`.

To reproduce the committed diff locally:

```bash
git diff --no-ext-diff --no-textconv --no-color --unified=0 \
  origin/master...HEAD -- > /tmp/tract-mutants.diff
cargo mutants -p tract-effects --config .cargo/mutants-tract-effects.toml \
  --in-diff /tmp/tract-mutants.diff
```

The diff's new text must match the working tree. Test-only, dependency, config,
and out-of-scope changes can select zero mutations and succeed without a baseline
build. Diff mode cannot detect reduced coverage of unchanged production code;
use a full run when reviewing test changes or coverage policy. Regular workspace
tests still run in the normal CI workflow.
