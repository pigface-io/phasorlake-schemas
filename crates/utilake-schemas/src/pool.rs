//! Work spread over threads, results in the order of the work.
//!
//! The census is bound by the CPU (every value of every row is profiled),
//! not by the disk, so files are read on several threads at once. Each
//! thread takes the next item off a shared counter, and every result goes
//! back to its item's place: the output is the same bytes whatever the
//! number of threads.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The threads a census uses by default: one per core the system offers
/// (eight on the machine the archive is read on), each reading its own file
/// in long sequential reads, which a spinning disk serves well. While
/// something else writes the same disk, `--jobs 6` leaves it and the
/// system room.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// `f` of every item, on up to `jobs` threads, in the items' order. `f`
/// gets the index of its thread, for anything a thread keeps to itself.
pub fn map<T: Sync, R: Send>(
    jobs: usize,
    items: &[T],
    f: impl Fn(usize, &T) -> R + Sync,
) -> Vec<R> {
    let jobs = jobs.clamp(1, items.len().max(1));
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, R)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..jobs)
            .map(|w| {
                let (next, f) = (&next, &f);
                s.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        out.push((i, f(w, item)));
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    done.sort_unstable_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_keep_the_order_of_the_work() {
        let items: Vec<u64> = (0..200).collect();
        for jobs in [0, 1, 3, 8, 500] {
            let out = map(jobs, &items, |_, x| x * x);
            assert_eq!(
                out,
                items.iter().map(|x| x * x).collect::<Vec<_>>(),
                "{jobs}"
            );
        }
        assert!(default_jobs() >= 1);
        let threads = map(4, &items, |w, _| w);
        assert!(threads.iter().all(|w| *w < 4));
        assert!(map(4, &[] as &[u8], |_, x| *x).is_empty());
    }

    #[test]
    #[should_panic(expected = "boom")]
    fn a_panic_in_the_work_is_the_callers() {
        map(
            2,
            &[1, 2, 3],
            |_, x| if *x == 2 { panic!("boom") } else { *x },
        );
    }
}
