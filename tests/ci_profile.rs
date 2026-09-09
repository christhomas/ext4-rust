//! The debug run that lets the PR gate see an overflow guards itself.
//!
//! `overflow-checks` is on in debug and off in release, so a defect
//! whose only symptom is an arithmetic overflow panic cannot be
//! observed by a release-only test run. This repository already runs a
//! debug suite -- `release.yml:59`, `cargo test --locked --all-targets`
//! -- and that is NOT the same fact as the pull-request gate being able
//! to see the defect: `release.yml` triggers on a version tag, after
//! the change has already merged. A wrapping bug merges green here and
//! surfaces only when someone else cuts the next release, detached from
//! the change and the person who could have caught it.
//!
//! So `ci.yml` -- the workflow that actually gates a merge -- needs its
//! own debug run, and this file is what keeps it there. It checks
//! `ci.yml` specifically and is not satisfied by `release.yml` having
//! one; see `a_debug_run_in_release_yml_alone_does_not_satisfy_the_gate`
//! for that distinction pinned as a test rather than left as a comment
//! someone could stop believing.
//!
//! # Why this is an integration test and not a module under `src/`
//!
//! Cargo discovers `tests/*.rs` on its own, so there is no declaration
//! anywhere that can be deleted to switch this off. A guard living as a
//! file under `src/` behind a `#[cfg(test)] mod` line has no such
//! protection: lose the one line and the file stays, compiles into
//! nothing, and asserts nothing, with no lint to say so. That happened
//! once already on a sibling repository's version of this fix.
//!
//! # The other half: does the debug run actually ask anything
//!
//! `ci.yml` quoting a debug command inside the comment explaining it
//! (see the comment above the step this file is pinning) means a scan
//! that ignored comments would keep passing after the step itself was
//! deleted. And a debug step that compiles but never checks anything is
//! costing a compile for nothing, so the scan also requires the
//! `EXPECT_OVERFLOW_CHECKS` handshake that arms
//! `overflow_checks::the_build_the_gate_asked_to_check_does_check` in
//! `src/lib.rs` -- the runtime half that actually performs an overflow
//! and fails if the build let it through. This file proves the step is
//! present and asked to check; it cannot prove the build can see the
//! overflow, which is what the runtime test is for. Neither is
//! redundant with the other: delete the step and the runtime test never
//! runs at all; keep the step but drop the variable and the runtime
//! test runs, finds nothing to check, and passes doing nothing.

use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a file the guard depends on, or fail.
///
/// Panics rather than returning `None` on purpose: a version of this
/// that skipped when the file was missing would reproduce the exact
/// blindness the guard exists to prevent.
fn read_or_panic(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}. This guard must fail rather than skip: a \
             version of it that returned early here would be the same \
             blindness it exists to prevent.",
            path.display()
        )
    })
}

/// Every `cargo test` invocation in a workflow that would be compiled
/// with overflow checks on AND asked to check for one.
///
/// Three things disqualify a line:
///
/// - it is a YAML comment. Load-bearing here, not defensive: `ci.yml`
///   quotes the debug command verbatim in the comment above the step
///   that runs it;
/// - it is an inline trailing comment on an otherwise-`--release` line;
/// - it passes `--release`, or names a profile explicitly.
///
/// And the run must carry `EXPECT_OVERFLOW_CHECKS=1` -- a debug step
/// that never asks the build anything buys nothing over deleting it.
fn checking_debug_runs(workflow: &str) -> Vec<String> {
    workflow
        .lines()
        .filter_map(|raw| {
            let line = raw.trim_start();
            if line.starts_with('#') {
                return None;
            }
            let command = line.split(" #").next().unwrap_or(line).trim();
            if !command.contains("cargo test") {
                return None;
            }
            if command.contains("--release") || command.contains("--profile") {
                return None;
            }
            if !command.contains("EXPECT_OVERFLOW_CHECKS=1") {
                return None;
            }
            Some(command.to_string())
        })
        .collect()
}

/// A workflow, structured just far enough to answer one question:
/// does this step's result actually gate a pull request?
///
/// The line-based scan above finds the command. It cannot see the
/// step's sibling keys, so `if: false` and `continue-on-error: true`
/// left every guard test green while the gate stopped gating -- a step
/// that runs and whose result nothing reads, which is this project's
/// own named defect, committed inside the guard written to prevent it.
///
/// The conditions are ENUMERATED rather than patched one defeat at a
/// time, because twice on this shape the defeat lived in what the scan
/// does not look at rather than in what it compares. A `run:` step
/// gates a pull request only if the step carries no `if:` and no
/// `continue-on-error:`, its job carries neither either, and the
/// workflow still triggers on `pull_request`.
///
/// `if:` and `continue-on-error:` are rejected on the KEY'S PRESENCE,
/// not by evaluating it. `if: false`, `if: ${{ false }}` and an `if:`
/// on an expression that happens to be false are distinct spellings,
/// and four spellings of one manifest key had already defeated a
/// matcher on a sibling repository -- enumerating them is the losing
/// game. Over-strict is the safe direction: a step that genuinely
/// needs a condition can be split out, whereas a guard that
/// interprets conditions acquires a new defeat whenever the syntax
/// grows.
///
/// A step's other keys -- `name:`, `env:`, `uses:`/`with:` -- say
/// nothing about whether the result is read, so they are ACCEPTED. An
/// `env:` mapping in particular must not disqualify a step: that would
/// be over-strictness in the one direction that costs something, since
/// the handshake this guard looks for is itself an environment
/// variable and a maintainer may reasonably move it into a mapping.
///
/// A `run:` whose value is a `|` block is joined into one string, so a
/// command inside a shell loop is seen whole rather than as fragments.
#[derive(Debug)]
struct Step {
    keys: Vec<String>,
    run: String,
}

#[derive(Debug)]
struct Job {
    keys: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Debug)]
struct Workflow {
    /// The trigger NAMES, parsed. Not the `on:` block's text: a
    /// substring search over that text answered `true` for
    /// `pull_request_review:` and for `pull_request` sitting inside a
    /// comment, so the guard reported pull-request coverage that was
    /// not there. Neither spelling needs an adversarial author.
    triggers: Vec<String>,
    jobs: Vec<Job>,
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A line with any trailing comment removed.
///
/// Only a `#` that starts a token counts, so a `#` inside a value --
/// `run: echo '#1'` -- is left alone. Crude next to real YAML, and in
/// the safe direction: a comment mistaken for content can only make
/// this parser see a key that is not there, which refuses a workflow
/// rather than approving one.
fn without_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return &line[..i];
        }
    }
    line
}

/// The key a mapping line declares, with any quotes removed.
///
/// `"if": false` and `'continue-on-error': true` are valid YAML and
/// GitHub Actions honours them exactly as the bare spellings, but a
/// raw text compare against `if` matches neither -- so a step that
/// does not gate was counted as one that does. The quotes are the
/// whole finding; everything else here is unchanged.
fn key_of(line: &str) -> Option<String> {
    let t = without_comment(line).trim();
    let t = t.strip_prefix("- ").unwrap_or(t);
    let t = t.trim_start();
    let (raw, _) = t.split_once(':')?;
    // The SAME normalisation `profiles_disabling_overflow_checks`
    // already applies to manifest keys in this file, after a quoted
    // `"overflow-checks" = false` defeated that scan. The lesson did
    // not travel the few dozen lines from the TOML parser to the YAML
    // one; it has now.
    let unquoted = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if unquoted.is_empty() {
        return None;
    }
    Some(unquoted.to_string())
}

/// Whether a `run:`'s value opens a block scalar rather than being the
/// command itself.
///
/// `|` is not the only spelling. YAML's chomping and indentation
/// indicators -- `|-`, `|+`, `>`, `>-`, `>+`, `|2`, `>2-` -- all open
/// a block, and treating one as the command meant the block's contents
/// were never read: a behaviour-preserving change from `|` to `|-`
/// made the guard report that nothing gated.
fn opens_a_block_scalar(value: &str) -> bool {
    let v = value.trim();
    let Some(rest) = v.strip_prefix('|').or_else(|| v.strip_prefix('>')) else {
        return false;
    };
    rest.chars()
        .all(|c| c == '-' || c == '+' || c.is_ascii_digit())
}

/// Structure a workflow far enough to answer the questions above.
///
/// Deliberately conservative: anything this cannot place confidently
/// is left out, so an unparsed step is a step that does not count. The
/// failure direction is a guard that refuses a workflow it did not
/// understand, which is loud, rather than one that approves it.
fn parse_workflow(text: &str) -> Workflow {
    let mut triggers: Vec<String> = Vec::new();
    let mut jobs: Vec<Job> = Vec::new();

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    // The `on:` block, parsed into trigger NAMES up to the next
    // top-level key. Comments are stripped and each name is matched
    // whole, so `pull_request_review:` and a commented-out
    // `# pull_request` are not `pull_request`.
    while i < lines.len() {
        let l = lines[i];
        if key_of(l).as_deref() == Some("on") && indent_of(l) == 0 {
            // `on: push` and `on: [push, pull_request]` both put the
            // triggers on this line.
            if let Some((_, after)) = without_comment(l).split_once(':') {
                let after = after.trim();
                let inner = after
                    .strip_prefix('[')
                    .and_then(|a| a.strip_suffix(']'))
                    .unwrap_or(after);
                for name in inner.split(',') {
                    let name = name.trim().trim_matches(|c| c == '"' || c == '\'').trim();
                    if !name.is_empty() {
                        triggers.push(name.to_string());
                    }
                }
            }
            i += 1;
            while i < lines.len() && (lines[i].trim().is_empty() || indent_of(lines[i]) > 0) {
                let line = without_comment(lines[i]);
                // A trigger is a key -- or a `- name` item -- at the
                // block's own indent. Anything deeper belongs to a
                // trigger's own options (`branches:`, `types:`) and is
                // not itself a trigger.
                if indent_of(line) == 2 {
                    if let Some(k) = key_of(line) {
                        triggers.push(k);
                    } else if let Some(item) = line.trim().strip_prefix("- ") {
                        let item = item.trim().trim_matches(|c| c == '"' || c == '\'').trim();
                        if !item.is_empty() {
                            triggers.push(item.to_string());
                        }
                    }
                }
                i += 1;
            }
            continue;
        }
        if indent_of(l) == 0 && key_of(l).as_deref() == Some("jobs") {
            i += 1;
            break;
        }
        i += 1;
    }

    // Jobs: each is a key at indent 2 under `jobs:`.
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            i += 1;
            continue;
        }
        let ind = indent_of(line);
        if ind == 0 {
            break; // another top-level key; jobs are done
        }
        if ind != 2 || !without_comment(line).trim_end().ends_with(':') {
            i += 1;
            continue;
        }
        // A job. Collect its keys and steps until the next indent-2 key.
        let mut job = Job {
            keys: Vec::new(),
            steps: Vec::new(),
        };
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            if !l.trim().is_empty() && indent_of(l) <= 2 && !l.trim_start().starts_with('#') {
                break;
            }
            let t = without_comment(l).trim_start();
            if indent_of(l) == 4 && !t.starts_with('#') && !t.starts_with('-') {
                if let Some(key) = key_of(l) {
                    job.keys.push(key);
                }
            }
            if indent_of(l) == 4 && key_of(l).as_deref() == Some("steps") {
                i += 1;
                // Steps: list items at some indent > 4.
                let mut item_indent: Option<usize> = None;
                while i < lines.len() {
                    let sl = lines[i];
                    if !sl.trim().is_empty()
                        && indent_of(sl) <= 4
                        && !sl.trim_start().starts_with('#')
                    {
                        break;
                    }
                    let st = sl.trim_start();
                    if st.starts_with("- ") {
                        let this_indent = indent_of(sl);
                        if item_indent.is_none() {
                            item_indent = Some(this_indent);
                        }
                        if Some(this_indent) == item_indent {
                            // A new step. Its keys sit at this_indent + 2.
                            let key_indent = this_indent + 2;
                            let mut step = Step {
                                keys: Vec::new(),
                                run: String::new(),
                            };
                            // First key is on the `- ` line itself.
                            let mut cur = st.trim_start_matches("- ").to_string();
                            let mut in_run = false;
                            loop {
                                let key = key_of(&cur).unwrap_or_default();
                                if !key.is_empty() {
                                    step.keys.push(key.clone());
                                }
                                if key == "run" {
                                    in_run = true;
                                    let after = without_comment(&cur)
                                        .split_once(':')
                                        .map(|x| x.1)
                                        .unwrap_or("")
                                        .trim();
                                    // A block scalar in ANY of its
                                    // spellings means the command is
                                    // on the following lines.
                                    if !opens_a_block_scalar(after) && !after.is_empty() {
                                        step.run.push_str(after);
                                        step.run.push('\n');
                                        in_run = false;
                                    }
                                } else if in_run {
                                    in_run = false;
                                }
                                i += 1;
                                if i >= lines.len() {
                                    break;
                                }
                                let nl = lines[i];
                                if nl.trim().is_empty() {
                                    continue;
                                }
                                let ni = indent_of(nl);
                                let nt = nl.trim_start();
                                if ni <= this_indent && !nt.starts_with('#') {
                                    break; // next step or end of steps
                                }
                                if in_run && ni > key_indent {
                                    step.run.push_str(nt);
                                    step.run.push('\n');
                                    continue;
                                }
                                if ni == key_indent && !nt.starts_with('#') {
                                    cur = nt.to_string();
                                    continue;
                                }
                                // Anything else (comments, a deeper mapping
                                // under a non-run key such as `env:` or
                                // `with:`) is skipped, which is what makes
                                // those keys accepted rather than fatal.
                            }
                            job.steps.push(step);
                            continue;
                        }
                    }
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
        jobs.push(job);
    }

    Workflow { triggers, jobs }
}

/// Does this workflow still run on a pull request at all?
///
/// The assumption the `ci.yml`-only scope rests on, and a fact about
/// the file rather than a given: if the triggers stop including
/// `pull_request`, the step gates nothing however it looks.
fn runs_on_pull_request(wf: &Workflow) -> bool {
    // MATCHED WHOLE, against parsed trigger names. A substring search
    // over the `on:` block's text answered `true` for
    // `pull_request_review:` -- which fires on review events, not on a
    // pull request opening or being pushed to, so it gates nothing --
    // and for `pull_request` inside a comment, including the comment
    // that says it was switched off.
    //
    // `pull_request_target` IS included, deliberately: it runs on pull
    // requests, in the base-repository context, and can be a required
    // check. It is named rather than matched by prefix.
    wf.triggers
        .iter()
        .any(|t| t == "pull_request" || t == "pull_request_target")
}

/// Keys whose presence on a step or job means its result does not gate.
const NON_GATING_KEYS: [&str; 2] = ["if", "continue-on-error"];

fn job_gates(job: &Job) -> bool {
    !job.keys
        .iter()
        .any(|k| NON_GATING_KEYS.contains(&k.as_str()))
}

fn step_gates(step: &Step) -> bool {
    !step
        .keys
        .iter()
        .any(|k| NON_GATING_KEYS.contains(&k.as_str()))
}

/// The checking debug runs of steps that ACTUALLY GATE a pull request.
///
/// This is the function the guard below asks, and the whole of the
/// difference: [`checking_debug_runs`] finds the command, this asks
/// whether anything reads its result. Adding `if: false` to the
/// guarded step in `ci.yml`, or `continue-on-error: true`, left all
/// the guard's tests green while the gate stopped gating. See #142.
fn gating_checking_debug_runs(workflow: &str) -> Vec<String> {
    let wf = parse_workflow(workflow);
    if !runs_on_pull_request(&wf) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for job in wf.jobs.iter().filter(|j| job_gates(j)) {
        for step in job.steps.iter().filter(|s| step_gates(s)) {
            out.extend(checking_debug_runs(&step.run));
        }
    }
    out
}

/// The guard. Reads `ci.yml` -- the workflow that gates a pull request
/// -- and refuses if nothing there compiles the overflow checks and
/// asks the build to prove it.
///
/// `ci.yml` specifically, not `release.yml`. `release.yml` already has
/// a debug run and always has; it does not run on a pull request, so
/// its presence says nothing about whether a merge was gated by it.
#[test]
fn the_pr_gate_still_tests_in_a_profile_that_can_see_an_overflow() {
    let path = manifest_dir()
        .join(".github")
        .join("workflows")
        .join("ci.yml");
    let workflow = read_or_panic(&path);

    let debug_runs = gating_checking_debug_runs(&workflow);
    assert!(
        !debug_runs.is_empty(),
        "no `cargo test` in {} runs without `--release` while setting \
         EXPECT_OVERFLOW_CHECKS=1 IN A STEP WHOSE RESULT GATES A PULL \
         REQUEST, so a defect whose only symptom is an \
         arithmetic overflow panic can merge without the PR gate ever \
         seeing it. release.yml already runs a debug suite, and that \
         does not help: it triggers on a version tag, after the change \
         has merged. If the debug step in ci.yml looked redundant beside \
         the release one, it is not -- see the comment above it.",
        path.display()
    );
}

/// THE DISTINCTION THIS REPOSITORY NEEDS THAT A PORTED COPY WOULD MISS.
///
/// A workflow carrying a checking debug run under a name other than
/// `ci.yml` -- `release.yml`, in this repository's own case -- must not
/// satisfy the guard. Simulated here with `release.yml`'s actual step
/// shape: a plain `cargo test --locked --all-targets` with no
/// `EXPECT_OVERFLOW_CHECKS`, because that workflow was never asked to
/// carry the handshake and does not need to -- it already runs in
/// debug, unconditionally, so nothing there was ever blind. The
/// scenario worth pinning is the near miss: even a hypothetical debug
/// run in `release.yml` that DID set the handshake would not make
/// `ci.yml`'s own absence of one acceptable, because `release.yml`
/// triggers too late to gate a merge.
#[test]
fn a_checking_debug_run_that_is_not_in_ci_yml_does_not_satisfy_this_guard() {
    let release_yml_shape = "\
jobs:
  release:
    steps:
      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --all-targets
";
    // This function only ever reads ci.yml in the real guard above; this
    // test pins that the PARSER itself would still count such a line if
    // handed the wrong file, so the guard's safety is coming from WHICH
    // FILE it opens -- a fact worth being explicit about, since a future
    // edit that widened the scan to every workflow would silently stop
    // catching this repository's actual defect.
    assert_eq!(
        checking_debug_runs(release_yml_shape),
        vec!["- run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --all-targets".to_string()],
        "the parser itself would count this line -- the guard's correctness \
         depends on scanning ci.yml and ci.yml alone, not on the parser \
         refusing this shape"
    );
}

/// Whether the step's result is READ, which the line-based parser
/// above cannot see. Six conditions, each with its own test, plus a
/// control asserting the unmodified shape IS counted so the others
/// cannot pass for the wrong reason.
mod gating {
    use super::gating_checking_debug_runs as gating;

    /// The shape that does gate. Every test below is this with one
    /// thing changed, so a failure here means the fixture is wrong
    /// rather than the property.
    const GATING: &str = "\
on:
  pull_request:
    branches: [main]
jobs:
  test:
    steps:
      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
";

    const STEP: &str = "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n";

    #[test]
    fn the_control_shape_gates() {
        assert_eq!(
            gating(GATING).len(),
            1,
            "the control must be counted, or every test below passes for the wrong reason"
        );
    }

    #[test]
    fn a_step_carrying_if_does_not_gate() {
        for condition in [
            "if: false",
            "if: ${{ false }}",
            "if: github.event_name == 'push'",
            "if: ${{ env.SOMETHING == 'yes' }}",
        ] {
            let yaml = GATING.replace(STEP, &format!("{STEP}        {condition}\n"));
            assert_ne!(yaml, GATING, "the mutation must actually apply");
            assert!(
                gating(&yaml).is_empty(),
                "a step carrying `{condition}` may or may not run, so it cannot be what \
                 makes the gate able to see an overflow. Rejected on the key's presence \
                 rather than by evaluating it -- the spellings are open-ended."
            );
        }
    }

    #[test]
    fn a_step_carrying_continue_on_error_does_not_gate() {
        let yaml = GATING.replace(STEP, &format!("{STEP}        continue-on-error: true\n"));
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert!(
            gating(&yaml).is_empty(),
            "the step runs and its failure is discarded, which is this project's own \
             named defect: a step that runs and whose result nothing reads"
        );
    }

    #[test]
    fn a_job_carrying_if_does_not_gate() {
        let yaml = GATING.replace("  test:\n", "  test:\n    if: false\n");
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert!(
            gating(&yaml).is_empty(),
            "the same reasoning one level up: a job that may not run cannot gate"
        );
    }

    #[test]
    fn a_job_carrying_continue_on_error_does_not_gate() {
        let yaml = GATING.replace("  test:\n", "  test:\n    continue-on-error: true\n");
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert!(
            gating(&yaml).is_empty(),
            "a job whose failure is discarded cannot gate, however sound its steps"
        );
    }

    /// The assumption the `ci.yml`-only scope rests on, which is a
    /// fact about the file rather than a given.
    #[test]
    fn a_workflow_that_no_longer_runs_on_pull_request_does_not_gate() {
        let yaml = GATING.replace(
            "  pull_request:\n    branches: [main]\n",
            "  push:\n    branches: [main]\n",
        );
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert!(
            gating(&yaml).is_empty(),
            "scoping the scan to ci.yml assumes ci.yml is what runs on a pull request; \
             if its triggers stop including pull_request, the step gates nothing no \
             matter how it looks"
        );
    }

    /// `pull_request_target` IS A PULL-REQUEST TRIGGER, so the
    /// substring match in `runs_on_pull_request` counting it is
    /// deliberate rather than sloppy.
    ///
    /// Pinned because it reads like a bug and was mistaken for one
    /// while witnessing this fix: a mutation replacing `pull_request:`
    /// with `pull_request_target:` left the guard green and looked
    /// like a survivor. It is not -- such a workflow still runs on
    /// pull requests, in the base-repository context, and can still be
    /// a required check. The defeat that matters is the trigger going
    /// away, which the test above covers by removing it.
    #[test]
    fn a_pull_request_target_trigger_still_gates() {
        let yaml = GATING.replace("  pull_request:\n", "  pull_request_target:\n");
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert_eq!(
            gating(&yaml).len(),
            1,
            "pull_request_target runs on a pull request too, so a step under it gates"
        );
    }

    /// THE OTHER DIRECTION, which is the one that costs something.
    ///
    /// A step's `env:` mapping says nothing about whether its result
    /// is read, so it must NOT disqualify the step. Over-strictness
    /// here would be self-defeating: the handshake this guard looks
    /// for is itself an environment variable, and a maintainer moving
    /// it into a mapping would turn the guard red on a workflow that
    /// gates perfectly well. Pinned so a later tightening of the
    /// non-gating key list cannot quietly swallow it.
    #[test]
    fn a_step_carrying_an_env_mapping_still_gates() {
        let yaml = GATING.replace(
            STEP,
            "      - env:\n          CARGO_TERM_COLOR: always\n        run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
        );
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert_eq!(
            gating(&yaml).len(),
            1,
            "an `env:` mapping is not a condition and does not discard a result, so the \
             step still gates. Rejecting it would be over-strict in the one direction \
             that breaks a working workflow."
        );
    }

    /// A TRAILING COMMENT ON THE JOB LINE MUST NOT LOSE THE JOB.
    ///
    /// The job header is recognised by its line ending in a colon, and
    /// `  test:  # the gate` does not -- so the job's steps were never
    /// collected and the guard reported that nothing gates. Loud
    /// rather than silent, but wrong, and on a workflow that gates
    /// perfectly well. The over-strict direction is the safe one for a
    /// CONDITION; it is not safe for a comment.
    #[test]
    fn a_job_line_with_a_trailing_comment_still_gates() {
        let yaml = GATING.replace("  test:\n", "  test:  # the pull-request gate\n");
        assert_ne!(yaml, GATING, "the mutation must actually apply");
        assert_eq!(
            gating(&yaml).len(),
            1,
            "a comment after the job's name says nothing about whether its result is read"
        );
    }

    /// A COMMENT IS NOT A TRIGGER. The substring search this replaced
    /// answered `true` for the comment that says the trigger was
    /// switched off, which is the most likely place the word appears
    /// on a workflow that no longer gates.
    #[test]
    fn a_commented_out_pull_request_trigger_does_not_gate() {
        for spelling in [
            "  push:\n    branches: [main]\n  # pull_request disabled for now\n",
            "  push:\n    branches: [main]\n    # was: pull_request\n",
            "  push: # replaces pull_request\n    branches: [main]\n",
        ] {
            let yaml = GATING.replace("  pull_request:\n    branches: [main]\n", spelling);
            assert_ne!(yaml, GATING, "the mutation must actually apply");
            assert!(
                yaml.contains("pull_request"),
                "precondition: the word must still be PRESENT, or this tests nothing -- \
                 the whole point is text that mentions it while not triggering on it"
            );
            assert!(
                gating(&yaml).is_empty(),
                "a workflow whose only mention of pull_request is a comment gates \
                 nothing. Spelling: {spelling:?}"
            );
        }
    }

    /// A DIFFERENT TRIGGER THAT STARTS THE SAME WAY IS A DIFFERENT
    /// TRIGGER. `pull_request_review` fires on review events, not on a
    /// pull request opening or being pushed to, so a step under it
    /// cannot be what gates the pull request.
    ///
    /// This is why the check matches whole names and lists
    /// `pull_request_target` explicitly rather than matching a prefix.
    #[test]
    fn a_similarly_named_trigger_does_not_gate() {
        for trigger in [
            "pull_request_review",
            "pull_request_review_comment",
            "pull_requests",
        ] {
            let yaml = GATING.replace("  pull_request:\n", &format!("  {trigger}:\n"));
            assert_ne!(yaml, GATING, "the mutation must actually apply");
            assert!(
                gating(&yaml).is_empty(),
                "`{trigger}` is not `pull_request`, and a substring match said it was"
            );
        }
    }

    /// A QUOTED KEY IS THE SAME KEY. `"if": false` is valid YAML and
    /// GitHub Actions honours it exactly as `if: false`, but a raw
    /// text compare against `if` matched neither quoted spelling -- so
    /// a step that does not gate was counted as one that does.
    ///
    /// This file's manifest parser already normalises quotes, after a
    /// quoted `"overflow-checks" = false` defeated that scan. Same
    /// defect one format across.
    #[test]
    fn a_quoted_non_gating_key_still_does_not_gate() {
        for key in [
            "\"if\": false",
            "'if': false",
            "\"continue-on-error\": true",
            "'continue-on-error': true",
        ] {
            let yaml = GATING.replace(STEP, &format!("{STEP}        {key}\n"));
            assert_ne!(yaml, GATING, "the mutation must actually apply");
            assert!(
                gating(&yaml).is_empty(),
                "a step carrying `{key}` does not gate, and the quotes do not change that"
            );
        }
    }

    /// The same, one level up, where the job-level key extraction had
    /// the identical bypass.
    #[test]
    fn a_quoted_non_gating_key_on_the_job_still_does_not_gate() {
        for key in ["\"if\": false", "'continue-on-error': true"] {
            let yaml = GATING.replace("  test:\n", &format!("  test:\n    {key}\n"));
            assert_ne!(yaml, GATING, "the mutation must actually apply");
            assert!(
                gating(&yaml).is_empty(),
                "a job carrying `{key}` does not gate, quoted or not"
            );
        }
    }

    /// EVERY BLOCK-SCALAR SPELLING IS A BLOCK. `|` is not the only
    /// one: YAML's chomping and indentation indicators all open a
    /// block, and treating one as the command itself meant the block's
    /// contents were never read -- so changing `|` to `|-`, which
    /// preserves behaviour, made the guard report that nothing gated.
    #[test]
    fn a_run_block_is_read_whole_in_every_block_scalar_spelling() {
        for indicator in ["|", "|-", "|+", ">", ">-", ">+", "|2"] {
            let yaml = format!(
                "on:\n  pull_request:\n    branches: [main]\njobs:\n  test:\n    steps:\n\
                 {}      - name: a block\n        run: {indicator}\n          set -euo pipefail\n\
                 {}          EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib\n",
                "", ""
            );
            assert_eq!(
                gating(&yaml).len(),
                1,
                "`run: {indicator}` opens a block, so the command inside it must be seen"
            );
        }
    }

    /// The flow-sequence and single-scalar spellings of `on:`, which
    /// put the triggers on the same line rather than in a block.
    #[test]
    fn the_inline_trigger_spellings_are_read_too() {
        let base = GATING.replace("on:\n  pull_request:\n    branches: [main]\n", "");
        for (spelling, gates) in [
            ("on: [push, pull_request]\n", true),
            ("on: [push]\n", false),
            ("on: pull_request\n", true),
            ("on: push\n", false),
            ("on:\n  - push\n  - pull_request\n", true),
            ("on:\n  - push\n", false),
        ] {
            let yaml = format!("{spelling}{base}");
            assert_eq!(
                !gating(&yaml).is_empty(),
                gates,
                "`{spelling:?}` should {} gate",
                if gates { "" } else { "not" }
            );
        }
    }

    /// A `run: |` block is read whole, so a command inside a shell
    /// loop is visible rather than seen as fragments.
    #[test]
    fn a_run_block_is_read_whole() {
        let yaml = "\
on:
  pull_request:
    branches: [main]
jobs:
  test:
    steps:
      - name: a block
        run: |
          set -euo pipefail
          EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
";
        assert_eq!(
            gating(yaml).len(),
            1,
            "a command inside a `run: |` block must be seen; several steps in this \
             repository's ci.yml are blocks like this one"
        );
    }

    /// The real `ci.yml` gates, read through the same function the
    /// guard uses. Distinct from the guard's own assertion: this one
    /// proves the PARSER copes with the real file's shape -- matrix
    /// strategies, `uses:`/`with:` mappings, comments between steps --
    /// rather than only with the fixtures above.
    #[test]
    fn the_real_ci_yml_still_parses_into_a_gating_step() {
        let workflow = super::read_or_panic(
            &super::manifest_dir()
                .join(".github")
                .join("workflows")
                .join("ci.yml"),
        );
        assert!(
            !gating(&workflow).is_empty(),
            "the real ci.yml must parse into at least one gating step, or the guard is \
             passing on a fixture and failing on the file it exists to read"
        );
    }
}

/// The parser is the part of this that can rot, checked against each
/// shape it has to tell apart.
mod parser {
    use super::checking_debug_runs;

    /// The trap this repository's own `ci.yml` contains: the debug
    /// command quoted verbatim in the comment explaining the step.
    #[test]
    fn a_debug_run_quoted_in_a_comment_does_not_count() {
        let yaml = "\
jobs:
  test:
    steps:
      # Measured on this branch:
      #     EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib   ->  EXIT=101
      - run: cargo test --locked --release
";
        assert_eq!(
            checking_debug_runs(yaml),
            Vec::<String>::new(),
            "a debug command quoted inside a comment is documentation, not a run"
        );
    }

    #[test]
    fn a_real_checking_debug_run_counts() {
        let yaml = "\
jobs:
  test:
    steps:
      - run: cargo test --locked --release
      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib
";
        assert_eq!(
            checking_debug_runs(yaml),
            vec!["- run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib".to_string()],
        );
    }

    /// A debug step present but never asked to check anything buys
    /// nothing over deleting it -- the whole point of the handshake.
    #[test]
    fn a_debug_run_without_the_handshake_does_not_count() {
        let yaml = "      - run: cargo test --locked --lib\n";
        assert_eq!(
            checking_debug_runs(yaml),
            Vec::<String>::new(),
            "the step runs but nothing checks the build it produced"
        );
    }

    /// A handshake on a release run proves nothing: the checks are
    /// legitimately off there.
    #[test]
    fn the_handshake_on_a_release_run_does_not_count() {
        let yaml = "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --release\n";
        assert_eq!(checking_debug_runs(yaml), Vec::<String>::new());
    }

    /// An inline trailing comment naming `--release` must not disqualify
    /// a genuine debug run.
    #[test]
    fn a_trailing_comment_naming_release_does_not_disqualify_a_debug_run() {
        let yaml =
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib  # deliberately not --release\n";
        assert_eq!(
            checking_debug_runs(yaml),
            vec!["- run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --lib".to_string()],
        );
    }

    /// A profile named another way still disqualifies the run.
    #[test]
    fn a_profile_flag_disqualifies_a_run_even_with_the_handshake() {
        let yaml =
            "      - run: EXPECT_OVERFLOW_CHECKS=1 cargo test --locked --profile release-with-debug --lib\n";
        assert_eq!(checking_debug_runs(yaml), Vec::<String>::new());
    }
}
