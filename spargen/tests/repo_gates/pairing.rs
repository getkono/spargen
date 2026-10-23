//! Each CI job held to the mise tasks it pairs with, every workflow classified, and what the
//! paired tasks and workflows may and may not do: write under a fixed `/tmp/` path, smudge LFS
//! content in the release preview, or miss a release's benchmarks.

use std::collections::{BTreeMap, BTreeSet};

use crate::repo::{mise_commands, mise_env, mise_tasks, read, workflow, yaml_document};
use crate::workflow_files;

/// How a CI job and the mise tasks relate. Every job in the [`GATE_WORKFLOWS`] and every task in
/// `mise.toml` is named by exactly one row of [`PAIRINGS`], so a new job or task cannot arrive
/// unclassified. There is no "not yet identical" row: every job is held to its tasks, and every
/// difference between the two is a named, literal exception inside a row.
enum Pairing {
    /// The job runs exactly what the tasks run; see [`Pair`].
    Identical(Pair),
    /// A task with no CI counterpart by nature: it rewrites the tree or installs hooks.
    LocalOnly {
        task: &'static str,
        why: &'static str,
    },
}

/// A job held to its tasks. The job's `run:` gate steps, in order and with their `env:`, are
/// exactly the tasks' `run` entries in the order listed, with each task's `env`, once each
/// [`Rewrite`] is applied to CI's side. Every other step is one of `ci_only`, in order, byte for
/// byte as parsed YAML.
struct Pair {
    /// The file under `.github/workflows/`.
    workflow: &'static str,
    job: &'static str,
    tasks: &'static [&'static str],
    ci_only: &'static [CiOnly],
    /// The job's `if:`, pinned literally, where the job has one.
    job_if: Option<&'static str>,
    rewrites: &'static [Rewrite],
}

const PAIR: Pair = Pair {
    workflow: "ci.yml",
    job: "",
    tasks: &[],
    ci_only: &[],
    job_if: None,
    rewrites: &[],
};

/// A step a job runs that its tasks do not, as the literal YAML of the step. Pinned whole, so
/// appending `&& cargo test` to a `run:`, adding `continue-on-error:` or `if:`, or moving a
/// toolchain's `@ref` all fail.
struct CiOnly {
    step: &'static str,
    /// Empty for provisioning: a `uses:` of one of [`PROVISIONING_ACTIONS`], with at most a
    /// `with:`. Otherwise this step is a named exception, and this says why mise has no
    /// counterpart for it.
    why: &'static str,
}

const fn provision(step: &'static str) -> CiOnly {
    CiOnly { step, why: "" }
}

const CHECKOUT: CiOnly = provision("uses: actions/checkout@v4");
const CHECKOUT_LFS: CiOnly = provision("uses: actions/checkout@v4\nwith:\n  lfs: true");
// These name `rust-toolchain.toml`'s `channel`, a concrete release, so a toolchain bump changes
// them together with the workflows (`ci_installs_the_rust_toolchain_this_file_pins`).
const STABLE: CiOnly = provision("uses: dtolnay/rust-toolchain@1.98.1");
const STABLE_CLIPPY: CiOnly =
    provision("uses: dtolnay/rust-toolchain@1.98.1\nwith:\n  components: clippy");
const STABLE_CLIPPY_WASM: CiOnly = provision(
    "uses: dtolnay/rust-toolchain@1.98.1\nwith:\n  components: clippy\n  targets: wasm32-unknown-unknown",
);
const CACHE: CiOnly = provision("uses: Swatinem/rust-cache@v2");

/// A named difference between CI's command and the task's: `ci` is replaced by `mise` in CI's
/// commands before they are compared, and `ci` must occur exactly once in them. Everything
/// outside `ci` is still compared byte for byte, and `ci` itself is pinned literally.
struct Rewrite {
    ci: &'static str,
    mise: &'static str,
    why: &'static str,
}

/// A workflow whose every job must be named by a [`PAIRINGS`] row, with the top-level keys that
/// decide when and whether its jobs run pinned literally (compared as parsed YAML). A `paths:`
/// filter under `pull_request:`, a narrower `branches:`, or a different `cancel-in-progress`
/// would make CI gate less than the tasks do without touching a single job.
struct GateWorkflow {
    file: &'static str,
    on: &'static str,
    /// `None` where the workflow has no `concurrency:`.
    concurrency: Option<&'static str>,
    /// `None` where the workflow has no `permissions:`.
    permissions: Option<&'static str>,
}

/// Every workflow under `.github/workflows/` (`.yml` or `.yaml`) is one of these or one of
/// [`NON_GATE_WORKFLOWS`]; an unclassified file fails, so a new workflow cannot run a gate no
/// pairing sees.
const GATE_WORKFLOWS: [GateWorkflow; 4] = [
    GateWorkflow {
        file: "ci.yml",
        on: "push:\n  branches: [master]\npull_request:",
        concurrency: Some("group: ci-${{ github.ref }}\ncancel-in-progress: true"),
        permissions: None,
    },
    // `ci.yml`'s triggers plus a daily schedule on `master` (and a manual one): the audit is
    // non-hermetic, so a new advisory must be found on `master` rather than by whichever open pull
    // request runs next (#146). Dropping `schedule:` fails here.
    GateWorkflow {
        file: "deny.yml",
        on: "push:\n  branches: [master]\npull_request:\nschedule:\n  - cron: \"17 6 * * *\"\nworkflow_dispatch:",
        concurrency: Some("group: deny-${{ github.ref }}\ncancel-in-progress: true"),
        permissions: None,
    },
    // On demand only as far as GitHub's triggers go: the release run is a dispatch from
    // `release-plz.yml` (`benchmarks_are_recorded_once_for_each_spargen_release`), because a tag
    // release-plz pushes with `GITHUB_TOKEN` starts no workflow (#685).
    GateWorkflow {
        file: "benchmarks.yml",
        on: "workflow_dispatch:",
        concurrency: None,
        permissions: None,
    },
    // Weekly and on demand only: a run takes hours, so no pull request waits on it (#490).
    // `cancel-in-progress: false` keeps a manual dispatch from cancelling a scheduled run.
    GateWorkflow {
        file: "mutants.yml",
        on: "schedule:\n  - cron: \"23 5 * * 1\"\nworkflow_dispatch:",
        concurrency: Some("group: mutants-${{ github.ref }}\ncancel-in-progress: false"),
        permissions: Some("contents: read"),
    },
];

/// Workflows that gate nothing, each with why. None of their jobs is paired, so a gate added to
/// one would run unseen: keep this list to workflows that are not gates by nature.
const NON_GATE_WORKFLOWS: [(&str, &str); 1] = [(
    "release-plz.yml",
    "publishes releases and maintains the release PR; it gates nothing a contributor could run \
     first",
)];

/// The top-level keys a gate workflow may carry. `name` is cosmetic, `env` is held to
/// [`COSMETIC_WORKFLOW_ENV`], and `on`, `concurrency` and `permissions` are pinned by the
/// workflow's [`GateWorkflow`] row. Anything else (`defaults:` can change the shell or directory
/// of every `run:` step without appearing on any of them) is rejected.
const WORKFLOW_KEYS: [&str; 6] = ["name", "on", "env", "concurrency", "permissions", "jobs"];

const PAIRINGS: &[Pairing] = &[
    Pairing::Identical(Pair {
        job: "fmt",
        tasks: &["fmt-check"],
        ci_only: &[
            CHECKOUT,
            provision("uses: dtolnay/rust-toolchain@1.98.1\nwith:\n  components: rustfmt"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "clippy",
        tasks: &["lint"],
        ci_only: &[CHECKOUT, STABLE_CLIPPY, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "test",
        tasks: &["test", "bench-build"],
        ci_only: &[CHECKOUT_LFS, STABLE_CLIPPY_WASM, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "check",
        tasks: &["check"],
        ci_only: &[CHECKOUT, STABLE, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "msrv",
        tasks: &["msrv"],
        ci_only: &[
            CHECKOUT,
            provision("uses: dtolnay/rust-toolchain@1.88.0"),
            CACHE,
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "powerset",
        tasks: &["powerset"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            CACHE,
            provision("uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-hack@0.6.39"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "runtime-dependencies",
        tasks: &["runtime-dependencies"],
        ci_only: &[
            CHECKOUT,
            provision("uses: dtolnay/rust-toolchain@nightly"),
            STABLE_CLIPPY_WASM,
            CACHE,
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "package",
        tasks: &["package"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            CACHE,
            CiOnly {
                step: "if: >-\n  (github.event_name == 'pull_request' && !startsWith(github.head_ref, 'release-plz-')) ||\n  (github.event_name == 'push' && !contains(github.event.head_commit.message, 'release-plz-'))\nrun: cargo publish --dry-run -p spargen-macro --config 'patch.crates-io.spargen.path=\"spargen\"'",
                why: "runs only outside release-plz PRs, a condition that exists only in CI; on a \
                      release PR the macro's registry dependency is not published yet",
            },
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "corpus-smoke",
        tasks: &["corpus-smoke"],
        ci_only: &[CHECKOUT_LFS, STABLE, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "github-api",
        tasks: &["github-api"],
        ci_only: &[CHECKOUT_LFS, STABLE_CLIPPY_WASM, CACHE],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "example",
        tasks: &["example"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision(
                "uses: Swatinem/rust-cache@v2\nwith:\n  workspaces: |\n    examples/petstore\n    examples/petstore-macro",
            ),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "docs",
        tasks: &["docs", "doc-links"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            CACHE,
            CiOnly {
                step: "name: Install mdBook\nrun: cargo install mdbook --version 0.5.4 --locked",
                why: "installs the mdBook that `[tools]` in mise.toml provisions locally",
            },
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "release-preview",
        tasks: &["release-preview"],
        ci_only: &[
            provision("uses: actions/checkout@v4\nwith:\n  fetch-depth: 0"),
            STABLE,
            provision("uses: taiki-e/install-action@v2\nwith:\n  tool: release-plz@0.3.160"),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "deny.yml",
        job: "deny",
        tasks: &["deny"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision(
                "uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-deny@0.20.2,cargo-audit@0.22.2",
            ),
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "deny.yml",
        job: "deny-published",
        tasks: &["deny-published"],
        ci_only: &[
            CHECKOUT,
            STABLE,
            provision(
                "uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-deny@0.20.2,cargo-audit@0.22.2",
            ),
        ],
        // The published artefact changes only when a release publishes, never with a pull
        // request's diff; failing pull requests on it would block the release pull request that
        // carries the fix (#178). Locally the task runs whenever it is asked for.
        job_if: Some("github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'"),
        ..PAIR
    }),
    Pairing::Identical(Pair {
        job: "commits",
        tasks: &["commit-range"],
        ci_only: &[
            provision("uses: actions/checkout@v4\nwith:\n  fetch-depth: 0"),
            CiOnly {
                step: "name: Install convco\nrun: cargo install convco --version 0.7.1 --locked",
                why: "installs the convco that `[tools]` in mise.toml provisions locally",
            },
        ],
        job_if: Some("github.event_name == 'pull_request'"),
        rewrites: &[Rewrite {
            ci: "${{ github.event.pull_request.base.sha }}..${{ github.event.pull_request.head.sha }}",
            mise: "origin/master..HEAD",
            why: "the outgoing range comes from the pull request event in CI and from the \
                  remote-tracking branch locally; the job runs only on pull requests for the \
                  same reason",
        }],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "benchmarks.yml",
        job: "bench",
        tasks: &["bench"],
        ci_only: &[
            CHECKOUT_LFS,
            STABLE,
            CACHE,
            CiOnly {
                step: "name: Upload benchmark results\nuses: actions/upload-artifact@v4\nwith:\n  name: benchmarks-${{ github.ref_name }}\n  path: |\n    bench-results.txt\n    target/criterion/**\n  if-no-files-found: error",
                why: "publishes the recorded results as the release artifact",
            },
        ],
        rewrites: &[
            Rewrite {
                ci: "set -o pipefail\n",
                mise: "",
                why: "a step's default `bash -e` has no `pipefail`, so without it a failing \
                      `cargo bench` would exit through `tee` with status 0; mise's command has \
                      no pipe to need it",
            },
            Rewrite {
                ci: " | tee bench-results.txt",
                mise: "",
                why: "captures the output for the artifact without changing what runs",
            },
        ],
        ..PAIR
    }),
    Pairing::Identical(Pair {
        workflow: "mutants.yml",
        job: "mutants",
        tasks: &["mutants"],
        ci_only: &[
            CHECKOUT_LFS,
            STABLE,
            CACHE,
            provision("uses: taiki-e/install-action@v2\nwith:\n  tool: cargo-mutants@27.1.0"),
        ],
        ..PAIR
    }),
    Pairing::LocalOnly {
        task: "fmt",
        why: "rewrites the tree; `fmt-check` is the gate CI mirrors",
    },
    Pairing::LocalOnly {
        task: "lint-fix",
        why: "rewrites the tree; `lint` is the gate CI mirrors",
    },
    Pairing::LocalOnly {
        task: "commit-msg",
        why: "validates one message as it is written; CI checks the range",
    },
    Pairing::LocalOnly {
        task: "hooks",
        why: "installs the git hooks",
    },
];

/// Actions a [`provision`] step may use. Each provisions a checkout, a toolchain, a cache, or a
/// binary; none runs a gate. An action outside this list may be a gate of its own (the
/// cargo-deny-action the `deny` job once ran was), which a `run:`-step comparison would never see.
const PROVISIONING_ACTIONS: [&str; 4] = [
    "actions/checkout@",
    "dtolnay/rust-toolchain@",
    "swatinem/rust-cache@",
    "taiki-e/install-action@",
];

/// Workflow-level `env:` entries that reach every job but change only how cargo prints, not what
/// it resolves or checks.
const COSMETIC_WORKFLOW_ENV: [&str; 1] = ["CARGO_TERM_COLOR"];

/// The keys a paired job may carry besides an `if:` its row pins. `runs-on` is held to
/// `ubuntu-latest` below. `timeout-minutes` is allowed unpinned: it bounds the runner's wall clock
/// and can only turn a run red, never green, so it cannot make CI narrower than the task. Every
/// other key (`needs`, `strategy`, `container`, `services`, `defaults`, `continue-on-error`, ...)
/// changes whether or how the steps run, and mise has nothing to match it with.
const JOB_KEYS: [&str; 5] = ["name", "runs-on", "steps", "env", "timeout-minutes"];

/// A YAML `env:` mapping as strings.
fn yaml_env(env: &yaml_rust2::Yaml, whose: &str) -> BTreeMap<String, String> {
    if env.is_badvalue() {
        return BTreeMap::new();
    }
    env.as_hash()
        .unwrap_or_else(|| panic!("{whose} `env:` must be a mapping"))
        .iter()
        .map(|(key, value)| {
            let key = key
                .as_str()
                .unwrap_or_else(|| panic!("{whose} `env:` keys must be strings"));
            let value = value.as_str().map(str::to_owned).unwrap_or_else(|| {
                panic!("{whose} `env.{key}` must be a string, as a mise task's `env` values are")
            });
            (key.to_owned(), value)
        })
        .collect()
}

/// The keys of a YAML mapping, as strings.
fn yaml_keys<'a>(map: &'a yaml_rust2::Yaml, whose: &str) -> Vec<&'a str> {
    map.as_hash()
        .unwrap_or_else(|| panic!("{whose} must be a mapping"))
        .keys()
        .map(|key| key.as_str().unwrap_or_default())
        .collect()
}

/// Holds a workflow's top level to what no task can see, and returns its `jobs:`.
fn gate_workflow(gate: &GateWorkflow) -> yaml_rust2::Yaml {
    let file = gate.file;
    let workflow = workflow(file);
    for key in yaml_keys(&workflow, &format!("`{file}`")) {
        assert!(
            WORKFLOW_KEYS.contains(&key),
            "`{file}` sets top-level `{key}:`, which can change whether or how every job runs; \
             no mise task has anything to match it with"
        );
    }
    for (key, pinned) in [
        ("on", Some(gate.on)),
        ("concurrency", gate.concurrency),
        ("permissions", gate.permissions),
    ] {
        let expected = pinned.map_or(yaml_rust2::Yaml::BadValue, |text| {
            yaml_document(text, &format!("`{file}`'s pinned `{key}:`"))
        });
        assert_eq!(
            workflow[key], expected,
            "`{file}`'s `{key}:` is not the one its GATE_WORKFLOWS row pins; a trigger filter or a \
             cancellation rule can skip a gate for a change the mise task would check"
        );
    }
    for key in yaml_env(&workflow["env"], &format!("`{file}`'s")).keys() {
        assert!(
            COSMETIC_WORKFLOW_ENV.contains(&key.as_str()),
            "`{file}` sets `{key}` for every job, which no mise task sets; either set it on each \
             task too or, if it only changes how output looks, list it in COSMETIC_WORKFLOW_ENV"
        );
    }
    workflow["jobs"].clone()
}

/// Checks `job`'s own keys and walks its steps: each is the next of `ci_only` (literally) or a
/// gate `run:` step. Returns the gate steps as (environment, trimmed command).
fn gate_steps(
    file: &str,
    name: &str,
    job: &yaml_rust2::Yaml,
    job_if: Option<&str>,
    ci_only: &[CiOnly],
) -> Vec<(BTreeMap<String, String>, String)> {
    assert!(!job.is_badvalue(), "`{file}` has no `{name}` job");
    for key in yaml_keys(job, &format!("the `{name}` job")) {
        assert!(
            JOB_KEYS.contains(&key) || (key == "if" && job_if.is_some()),
            "the `{name}` job in `{file}` sets `{key}:`, which changes whether or how its steps \
             run; its mise counterpart has nothing to match it with"
        );
    }
    assert_eq!(
        job["if"].as_str(),
        job_if,
        "the `{name}` job's `if:` is not the one its PAIRINGS row pins"
    );
    assert_eq!(
        job["runs-on"].as_str(),
        Some("ubuntu-latest"),
        "the `{name}` job must run on `ubuntu-latest`, the platform its tasks are held on"
    );
    let job_env = yaml_env(&job["env"], &format!("the `{name}` job's"));

    let mut expected = ci_only
        .iter()
        .map(|pinned| {
            let step = yaml_document(pinned.step, &format!("a `{name}` CI-only step"));
            if pinned.why.is_empty() {
                let keys = yaml_keys(&step, &format!("a `{name}` provisioning step"));
                assert!(
                    keys.iter().all(|key| ["uses", "with"].contains(key)),
                    "the `{name}` provisioning step `{}` carries more than `uses:` and `with:`; \
                     give it a `why` as a named exception",
                    pinned.step
                );
                let uses = step["uses"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                assert!(
                    PROVISIONING_ACTIONS
                        .iter()
                        .any(|action| uses.starts_with(action)),
                    "the `{name}` provisioning step uses `{uses}`, which is not a provisioning \
                     action; if it gates anything, the mise task never runs it"
                );
            }
            (pinned.step, step)
        })
        .peekable();

    let mut gates = Vec::new();
    for step in job["steps"]
        .as_vec()
        .unwrap_or_else(|| panic!("the `{name}` job must carry a list of steps"))
    {
        if expected.peek().is_some_and(|(_, pinned)| pinned == step) {
            expected.next();
            continue;
        }
        let keys = yaml_keys(step, &format!("a `{name}` step"));
        if let Some(uses) = step["uses"].as_str() {
            panic!(
                "the `{name}` job uses `{uses}` in a step no CI-only row pins literally; pin it in \
                 the job's PAIRINGS row (its `@ref` and `with:` included)"
            );
        }
        let run = step["run"]
            .as_str()
            .unwrap_or_else(|| panic!("a `{name}` step has neither `uses:` nor `run:`"));
        for key in keys {
            assert!(
                ["name", "run", "env"].contains(&key),
                "the `{name}` job's `{run}` step sets `{key}:`, which changes whether, where, or \
                 how it runs; its mise counterpart has nothing to match it with"
            );
        }
        let mut env = job_env.clone();
        env.extend(yaml_env(&step["env"], &format!("a `{name}` step's")));
        gates.push((env, run.trim().to_owned()));
    }
    if let Some((pinned, _)) = expected.next() {
        panic!(
            "the `{name}` job has no step `{pinned}` where its PAIRINGS row pins one (the pinned \
             steps must appear in order, byte for byte)"
        );
    }
    gates
}

#[test]
fn every_mise_task_runs_exactly_what_its_ci_job_runs() {
    // CI spells each gate out rather than calling `mise run`, so every gate exists twice. The
    // policy is that the two copies are identical -- the same commands with the same flags, in
    // the same order, under the same environment -- and neither is stricter or narrower than the
    // other. #141 was the deny gate auditing a smaller graph locally than in CI; the same audit
    // found `corpus-smoke` letting a crashed `check` through to its `grep` locally, and
    // `example`'s leak check discarding a failing `cargo tree` locally, where CI failed on both.
    //
    // Commands are compared as strings after trimming, so an identical gate means byte-identical
    // commands. mise runs each `run` entry under `sh -c -o errexit` and CI each step under
    // `bash -e`, so a command that is byte-identical also fails the same way in both. Every step
    // that is not compared -- checkouts, toolchains, caches, tool installs, and the few named
    // exceptions -- is pinned as literal YAML in its row, so none of them can grow a gate, a
    // condition, or a different toolchain unseen.
    let tasks = mise_tasks();
    let mut paired_jobs = BTreeSet::new();
    let mut paired_tasks = BTreeSet::new();
    let mut claim = |jobs: &[(&'static str, &'static str)], tasks: &[&'static str]| {
        for job in jobs {
            assert!(paired_jobs.insert(*job), "job {job:?} is paired twice");
        }
        for task in tasks {
            assert!(paired_tasks.insert(*task), "task `{task}` is paired twice");
        }
    };
    for pairing in PAIRINGS {
        match pairing {
            Pairing::Identical(pair) => claim(&[(pair.workflow, pair.job)], pair.tasks),
            Pairing::LocalOnly { task, why } => {
                assert!(!why.is_empty(), "a local-only task must say why");
                claim(&[], &[*task]);
            }
        }
    }
    let classified: BTreeSet<String> = GATE_WORKFLOWS
        .iter()
        .map(|gate| gate.file)
        .chain(NON_GATE_WORKFLOWS.iter().map(|(file, why)| {
            assert!(!why.is_empty(), "a non-gate workflow must say why");
            *file
        }))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        workflow_files(),
        classified,
        "every workflow under `.github/workflows/` must be a GATE_WORKFLOWS row (its jobs paired \
         with mise tasks) or a NON_GATE_WORKFLOWS entry saying why it gates nothing, and every \
         listed file must exist"
    );
    let mut workflow_jobs = BTreeMap::new();
    let mut ci_jobs = BTreeSet::new();
    for gate in &GATE_WORKFLOWS {
        let file = gate.file;
        let jobs = gate_workflow(gate);
        for job in jobs
            .as_hash()
            .unwrap_or_else(|| panic!("`{file}` must carry a `jobs:` mapping"))
            .keys()
        {
            ci_jobs.insert((file, job.as_str().expect("job ids are strings").to_owned()));
        }
        workflow_jobs.insert(file, jobs);
    }
    let mise_tasks: BTreeSet<&str> = tasks.keys().map(String::as_str).collect();
    let paired_jobs: BTreeSet<(&str, String)> = paired_jobs
        .into_iter()
        .map(|(file, job)| (file, job.to_owned()))
        .collect();
    assert_eq!(
        ci_jobs, paired_jobs,
        "every job in a GATE_WORKFLOWS workflow must be named by one PAIRINGS row, and every row's job \
         must exist"
    );
    assert_eq!(
        mise_tasks, paired_tasks,
        "every mise task must be named by one PAIRINGS row, and every row's task must exist"
    );

    for pairing in PAIRINGS {
        let pair = match pairing {
            Pairing::Identical(pair) => pair,
            Pairing::LocalOnly { .. } => continue,
        };
        let name = pair.job;
        let mut ci = gate_steps(
            pair.workflow,
            name,
            &workflow_jobs[pair.workflow][name],
            pair.job_if,
            pair.ci_only,
        );
        for rewrite in pair.rewrites {
            assert!(!rewrite.why.is_empty(), "a rewrite must say why");
            let found: usize = ci
                .iter()
                .map(|(_, command)| command.matches(rewrite.ci).count())
                .sum();
            assert_eq!(
                found, 1,
                "the `{name}` job's commands carry `{}` {found} times; its PAIRINGS row rewrites \
                 it exactly once, so CI's side of that exception has changed",
                rewrite.ci
            );
            for (_, command) in &mut ci {
                *command = command.replace(rewrite.ci, rewrite.mise).trim().to_owned();
            }
        }

        let local: Vec<(BTreeMap<String, String>, String)> = pair
            .tasks
            .iter()
            .flat_map(|task| {
                let env = mise_env(&tasks, task);
                mise_commands(&tasks, task)
                    .into_iter()
                    .map(move |command| (env.clone(), command.trim().to_owned()))
            })
            .collect();

        assert_eq!(
            local,
            ci,
            "`mise run {}` does not run exactly what `{}`'s `{name}` job runs (left: mise, right: \
             CI after its row's named rewrites; each entry is its environment and its command). \
             The two must be identical -- change whichever side is wrong, not only one of them",
            pair.tasks.join("` + `mise run "),
            pair.workflow,
        );
    }
}

#[test]
fn the_corpus_smoke_gate_writes_only_inside_the_checkout() {
    // A fixed name under the shared /tmp is owned by whoever ran the gate first; on a sticky
    // /tmp every later user's redirect fails with `Permission denied` before spargen runs
    // (#93). Both copies of the gate write under the gitignored `target/corpus-smoke/`: every
    // redirect either copy makes targets a path there, and neither file names `/tmp/` at all.
    for file in ["mise.toml", ".github/workflows/ci.yml"] {
        let text = read(file);
        assert!(
            !text.contains("/tmp/"),
            "`{file}` writes to a fixed `/tmp/` path; corpus-smoke outputs belong under `target/corpus-smoke/`"
        );
    }
    let ci: Vec<String> = workflow("ci.yml")["jobs"]["corpus-smoke"]["steps"]
        .as_vec()
        .expect("ci.yml carries a `corpus-smoke` job with steps")
        .iter()
        .filter_map(|step| step["run"].as_str().map(str::to_owned))
        .collect();
    for (gate, commands) in [
        (
            "`mise run corpus-smoke`",
            mise_commands(&mise_tasks(), "corpus-smoke"),
        ),
        ("the CI corpus-smoke job", ci),
    ] {
        let targets: Vec<&str> = commands
            .iter()
            .flat_map(|command| redirect_targets(command))
            .collect();
        assert!(
            !targets.is_empty(),
            "{gate} redirects no output; the scan has stopped reading its commands"
        );
        for target in targets {
            assert!(
                target.starts_with("target/corpus-smoke/")
                    && !target.split('/').any(|segment| segment == ".."),
                "{gate} redirects into `{target}`; corpus-smoke outputs belong under \
                 `target/corpus-smoke/`"
            );
        }
    }
    assert_eq!(
        redirect_targets("a > target/x.json 2>&1 || b >>/tmp/y 2> z"),
        ["target/x.json", "/tmp/y", "z"],
        "`redirect_targets` reads every `>`, `>>` and `N>` redirect, and no `>&` duplication"
    );
}

/// The file each output redirect in the shell `command` writes to: the word after each `>` or
/// `>>`, with or without a file-descriptor prefix or a space before it. A `>&N` duplication writes
/// no file and is skipped.
fn redirect_targets(command: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut rest = command;
    while let Some(at) = rest.find('>') {
        rest = rest[at + 1..].trim_start_matches('>');
        if rest.starts_with('&') {
            continue;
        }
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        targets.push(&rest[..end]);
        rest = &rest[end..];
    }
    targets
}

#[test]
fn benchmarks_are_recorded_once_for_each_spargen_release() {
    // `benchmarks.yml` once filtered on `push: tags: ["v*"]`, which matched none of release-plz's
    // `spargen-v*` / `spargen-macro-v*` tags, so it never ran on a release (#685). Fixing the glob
    // would not have been enough: release-plz pushes its tags with `GITHUB_TOKEN`, and GitHub
    // starts no workflow for an event that token causes, `workflow_dispatch` aside. So the release
    // job hands its `releases` output to a job that dispatches the benchmarks on the `spargen`
    // package's tag, whatever release-plz names it.
    let release = workflow("release-plz.yml");
    let jobs = &release["jobs"];
    let release_job = &jobs["release-plz-release"];
    assert_eq!(
        release_job["outputs"],
        yaml_document(
            "releases_created: ${{ steps.release-plz.outputs.releases_created }}\n\
             releases: ${{ steps.release-plz.outputs.releases }}",
            "the release job's pinned `outputs:`",
        ),
        "`release-plz.yml`'s release job must expose release-plz's `releases_created` and \
         `releases` outputs, which the benchmark dispatch reads"
    );
    let release_steps: Vec<_> = release_job["steps"]
        .as_vec()
        .expect("the release job carries steps")
        .iter()
        .filter(|step| step["id"].as_str() == Some("release-plz"))
        .collect();
    assert!(
        matches!(release_steps.as_slice(), [step] if step["with"]["command"].as_str() == Some("release")),
        "exactly one step of the release job, the one running `release-plz release`, must carry \
         `id: release-plz`, the step the job's outputs read"
    );
    assert_eq!(
        jobs["benchmarks"],
        yaml_document(
            "name: Dispatch release benchmarks\n\
             needs: release-plz-release\n\
             if: needs.release-plz-release.outputs.releases_created == 'true'\n\
             runs-on: ubuntu-latest\n\
             permissions:\n  actions: write\n\
             steps:\n\
             \x20 - name: Dispatch the benchmarks on the spargen release tag\n\
             \x20   env:\n\
             \x20     GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}\n\
             \x20     RELEASES: ${{ needs.release-plz-release.outputs.releases }}\n\
             \x20   run: |\n\
             \x20     tag=$(jq -r '.[]? | select(.package_name == \"spargen\") | .tag' <<< \"${RELEASES:-[]}\")\n\
             \x20     if [ -n \"$tag\" ]; then\n\
             \x20       gh workflow run benchmarks.yml --repo \"$GITHUB_REPOSITORY\" --ref \"$tag\"\n\
             \x20     fi\n",
            "the pinned benchmark dispatch job",
        ),
        "`release-plz.yml`'s `benchmarks` job is the only thing that runs the benchmarks on a \
         release: it must dispatch `benchmarks.yml` on the `spargen` tag of each release"
    );
    // The dispatch is what `benchmarks.yml` must accept; its `on:` is pinned by its GATE_WORKFLOWS
    // row, and this holds that row to the dispatch rather than to a filter that can never fire.
    let benchmarks = workflow("benchmarks.yml");
    assert!(
        benchmarks["on"]["workflow_dispatch"] != yaml_rust2::Yaml::BadValue,
        "`benchmarks.yml` must accept the `workflow_dispatch` `release-plz.yml` sends"
    );
    // Every tag in this repository is release-plz's, pushed with `GITHUB_TOKEN`, so a tag or
    // release trigger on any workflow is one that never fires.
    for file in workflow_files() {
        let on = &workflow(&file)["on"];
        for (event, filter) in [("push", "tags"), ("push", "tags-ignore")] {
            assert!(
                on[event][filter] == yaml_rust2::Yaml::BadValue,
                "`{file}` triggers on `{event}: {filter}:`; release-plz pushes every tag here with \
                 `GITHUB_TOKEN`, which starts no workflow, so dispatch the run from \
                 `release-plz.yml` as its `benchmarks` job does"
            );
        }
        for event in ["create", "release"] {
            assert!(
                on[event] == yaml_rust2::Yaml::BadValue,
                "`{file}` triggers on `{event}:`; release-plz creates every tag and release here \
                 with `GITHUB_TOKEN`, which starts no workflow, so dispatch the run from \
                 `release-plz.yml` as its `benchmarks` job does"
            );
        }
    }
}

#[test]
fn the_release_preview_never_smudges_lfs_content() {
    // The preview clones this checkout, and `release-plz update` then checks out other revisions
    // and this branch again; each checkout smudges every LFS file that differs, fetching it from
    // the clone's `origin`, which is this checkout. CI's checkout holds no LFS objects, so a pull
    // request that added a corpus file failed the preview with "remote missing object" (#357)
    // while `GIT_LFS_SKIP_SMUDGE=1` covered only the clone. No packaged file is an LFS object, so
    // every command that checks files out skips it. The pairing test holds CI to the same lines.
    let tasks = mise_tasks();
    let task_env = mise_env(&tasks, "release-preview");
    let mut checked = Vec::new();
    for line in mise_commands(&tasks, "release-preview") {
        for segment in line.split("&&") {
            let words: Vec<&str> = segment.split_whitespace().collect();
            let program = words
                .iter()
                .position(|word| !word.contains('='))
                .unwrap_or(words.len());
            let (assignments, command) = words.split_at(program);
            let checks_out = match command {
                ["release-plz", ..] => true,
                ["git", rest @ ..] => rest.contains(&"clone"),
                _ => false,
            };
            if !checks_out {
                continue;
            }
            let skipped = assignments.contains(&"GIT_LFS_SKIP_SMUDGE=1")
                || task_env.get("GIT_LFS_SKIP_SMUDGE").map(String::as_str) == Some("1");
            assert!(
                skipped,
                "`mise run release-preview` runs `{}` without `GIT_LFS_SKIP_SMUDGE=1`: it would \
                 fetch LFS content from a checkout that holds none",
                segment.trim()
            );
            checked.push(command[0].to_owned());
        }
    }
    assert!(
        ["git", "release-plz"]
            .iter()
            .all(|program| checked.iter().any(|seen| seen == program)),
        "the release preview no longer clones and runs release-plz ({checked:?}); revisit this test"
    );
}
