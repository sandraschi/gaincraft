//! Build facts for Help > About and Help > Licenses & Notices:
//! - `GAIN_COMMIT`: short git SHA ("unknown" outside a git checkout)
//! - `GAIN_BUILT`: build date (UTC, YYYY-MM-DD; honours SOURCE_DATE_EPOCH)
//! - `$OUT_DIR/third_party.tsv`: every registry crate in Cargo.lock with the
//!   license declared in its own Cargo.toml (name, version, license).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GAIN_COMMIT={commit}");
    println!("cargo:rustc-env=GAIN_BUILT={}", build_date());

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let root = manifest.join("..").join("..");
    let lock = root.join("Cargo.lock");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_else(|_| ".".into()));
    let rows = std::fs::read_to_string(&lock)
        .map(|t| third_party(&t))
        .unwrap_or_default();
    let tsv: String = rows
        .iter()
        .map(|(n, v, l)| format!("{n}\t{v}\t{l}\n"))
        .collect();
    if let Err(e) = std::fs::write(out.join("third_party.tsv"), tsv) {
        println!("cargo:warning=third_party.tsv not written: {e}");
    }
    println!("cargo:rerun-if-changed={}", lock.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git").join("HEAD").display()
    );
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}

fn build_date() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    let (y, m, d) = civil(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn registry_roots() -> Vec<PathBuf> {
    let home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(|h| Path::new(&h).join(".cargo")))
        .or_else(|_| std::env::var("HOME").map(|h| Path::new(&h).join(".cargo")))
        .unwrap_or_default();
    std::fs::read_dir(home.join("registry").join("src"))
        .map(|it| it.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

fn license_of(roots: &[PathBuf], name: &str, version: &str) -> String {
    for r in roots {
        let toml = r.join(format!("{name}-{version}")).join("Cargo.toml");
        if let Ok(t) = std::fs::read_to_string(&toml) {
            let mut in_package = false;
            for line in t.lines() {
                let l = line.trim();
                if l.starts_with('[') {
                    in_package = l == "[package]";
                    continue;
                }
                if in_package {
                    if let Some(v) = l.strip_prefix("license") {
                        let v = v.trim_start();
                        if let Some(v) = v.strip_prefix('=') {
                            return v.trim().trim_matches('"').to_string();
                        }
                    }
                    if l.starts_with("license-file") {
                        return "see license file".into();
                    }
                }
            }
            return "not declared".into();
        }
    }
    "see crates.io".into()
}

/// Registry packages in Cargo.lock (path crates of this workspace are skipped).
fn third_party(lock: &str) -> Vec<(String, String, String)> {
    let roots = registry_roots();
    let mut out = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let field = |k: &str| {
            block.lines().find_map(|l| {
                l.trim()
                    .strip_prefix(k)
                    .map(str::trim_start)
                    .and_then(|r| r.strip_prefix('='))
                    .map(|v| v.trim().trim_matches('"').to_string())
            })
        };
        let (Some(name), Some(version), Some(source)) =
            (field("name"), field("version"), field("source"))
        else {
            continue;
        };
        if !source.starts_with("registry+") {
            continue;
        }
        let lic = license_of(&roots, &name, &version);
        out.push((name, version, lic));
    }
    out.sort();
    out
}
