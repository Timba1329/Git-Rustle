//! Performance smoke test on a large history. Run with:
//!   cargo test -p rustle-core --test perf --release -- --ignored --nocapture

use rustle_core::{CliBackend, GitRunner, RunOpts, build_graph};
use std::time::Instant;

#[test]
#[ignore = "slow; run manually"]
fn log_and_graph_of_20k_commits() {
    const N: usize = 20_000;
    let dir = tempfile::tempdir().unwrap();
    let runner = GitRunner::discover().unwrap();
    runner
        .run(&["init", "-q", "-b", "main"], Some(dir.path()))
        .unwrap();

    // Build the history with `git fast-import`: a mostly linear chain with a side branch every 50 commits.
    let mut s = String::new();
    for i in 0..N {
        let msg = format!("commit number {i}");
        let body = format!("content {i}\n");
        s.push_str(&format!(
            "commit refs/heads/main\nmark :{}\ncommitter T <t@x.nl> {} +0000\ndata {}\n{msg}\n",
            i + 1,
            1_700_000_000 + i,
            msg.len()
        ));
        if i > 0 {
            s.push_str(&format!("from :{i}\n"));
        }
        s.push_str(&format!("M 644 inline f.txt\ndata {}\n{body}\n", body.len()));
    }
    runner
        .run_opts(
            &["fast-import", "--quiet"],
            Some(dir.path()),
            &RunOpts {
                input: Some(s.as_bytes()),
                ..Default::default()
            },
        )
        .unwrap();

    let b = CliBackend::open(runner, dir.path()).unwrap();
    let t = Instant::now();
    let commits = b.log(N, 0).unwrap();
    let t_log = t.elapsed();
    let t = Instant::now();
    let g = build_graph(&commits);
    let t_graph = t.elapsed();
    let t = Instant::now();
    let st = b.status().unwrap();
    let t_status = t.elapsed();
    println!(
        "log {N}: {t_log:?}, graph: {t_graph:?}, status: {t_status:?}, lanes: {}",
        g.width
    );
    assert_eq!(commits.len(), N);
    assert!(st.files.is_empty() || st.files.len() < 5);
    assert!(t_log.as_secs() < 10 && t_graph.as_secs() < 2);
}
