use crate::files;
use std::{fs, io, path::Path, time::Instant};

pub fn run(path: Option<&Path>) -> io::Result<()> {
    let owned = std::env::temp_dir().join(format!("mdir-bench-{}", std::process::id()));
    if path.is_none() {
        fs::create_dir(&owned)?;
    }
    let directory = path.unwrap_or(&owned);
    let result = (|| {
        if path.is_none() {
            for i in 0..20_000 {
                fs::write(directory.join(format!("항목-{i:06}.txt")), b"fixture")?;
            }
        }
        let _ = files::scan(directory)?;
        println!(
            "workload=sorted names/types; runs=7; warm cache; alternating order; no render/cold I/O; DOS original not measured"
        );
        let mut fast_times = Vec::new();
        let mut slow_times = Vec::new();
        for round in 0..7 {
            let (fast, slow, optimized, naive) = if round % 2 == 0 {
                let t = Instant::now();
                let a = files::scan(directory)?;
                let fast = t.elapsed();
                let t = Instant::now();
                let b = naive_scan(directory)?;
                (fast, t.elapsed(), a, b)
            } else {
                let t = Instant::now();
                let b = naive_scan(directory)?;
                let slow = t.elapsed();
                let t = Instant::now();
                let a = files::scan(directory)?;
                (t.elapsed(), slow, a, b)
            };
            if optimized
                .iter()
                .map(|e| (&e.name, e.directory))
                .ne(naive.iter().map(|(name, dir)| (name, *dir)))
            {
                return Err(io::Error::other(
                    "Benchmark output mismatch; comparison is invalid",
                ));
            }
            println!(
                "run={} entries={} optimized_ms={:.3} baseline_ms={:.3} equal=true",
                round + 1,
                optimized.len(),
                fast.as_secs_f64() * 1000.,
                slow.as_secs_f64() * 1000.
            );
            fast_times.push(fast);
            slow_times.push(slow);
            std::hint::black_box((optimized, naive));
        }
        fast_times.sort();
        slow_times.sort();
        println!(
            "median_optimized_ms={:.3} median_baseline_ms={:.3} ratio={:.2}",
            fast_times[3].as_secs_f64() * 1000.,
            slow_times[3].as_secs_f64() * 1000.,
            slow_times[3].as_secs_f64() / fast_times[3].as_secs_f64()
        );
        Ok(())
    })();
    if path.is_none() {
        fs::remove_dir_all(&owned)?;
    }
    result
}
fn naive_scan(path: &Path) -> io::Result<Vec<(std::ffi::OsString, bool)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        entries.push((
            entry.file_name(),
            fs::symlink_metadata(entry.path())?.is_dir(),
        ));
    }
    entries.sort_unstable_by(|a, b| {
        b.1.cmp(&a.1)
            .then(
                files::safe_text(&a.0.to_string_lossy())
                    .to_lowercase()
                    .cmp(&files::safe_text(&b.0.to_string_lossy()).to_lowercase()),
            )
            .then(a.0.cmp(&b.0))
    });
    Ok(entries)
}
