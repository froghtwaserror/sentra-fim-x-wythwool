
use std::{fs, io::Write};
use tempfile::tempdir;
use sentra_fim::{config::Config, fim};

#[test]
fn baseline_and_scan_jsonl() {
    // prepare temp fs
    let dir = tempdir().unwrap();
    let p = dir.path().join("a.txt");
    let mut f = fs::File::create(&p).unwrap();
    writeln!(f, "hello").unwrap();

    // config
    let cfg = Config {
        baseline_db: dir.path().join("base.db").to_string_lossy().to_string(),
        metrics_bind: "127.0.0.1:0".to_string(),
        watch_paths: vec![dir.path().to_string_lossy().to_string()],
        exclude: vec![],
        hash_alg: "blake3".to_string(),
        debounce_ms: 10,
    };

    // baseline
    fim::build_baseline(&cfg).unwrap();

    // modify file
    let mut f2 = fs::OpenOptions::new().append(true).open(&p).unwrap();
    writeln!(f2, "world").unwrap();

    // scan diff -> jsonl
    let jsonl = dir.path().join("diff.jsonl");
    fim::scan_diff(&cfg, Some(jsonl.to_string_lossy().to_string())).unwrap();

    let content = fs::read_to_string(jsonl).unwrap();
    assert!(content.contains("\"kind\":\"changed\"") || content.contains("\"kind\":\"added\""));
}
