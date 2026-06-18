//! Per-file output-size estimation for the pre-flight summary.
//!
//! The goal is to predict, before any encoding runs, how large the compressed
//! library will be. The naive approach — one flat ratio applied to every byte —
//! ignores that savings are **bitrate- and maxrate-dependent**: fat low-complexity
//! clips shrink ~8×, already-lean clips barely shrink, and a bitrate ceiling caps
//! the fattest ones (see CLAUDE.md → *Gotchas*).
//!
//! This module models output per file from facts ffprobe gives us cheaply
//! (duration, frame rate) plus the source byte size from the scan. The model is
//! anchored on the **blended** compaction actually measured on the reference
//! library (187.6 GB → 28.9 GB ≈ 6.5× at cq30), so the *summed* estimate over a
//! whole library is accurate even though any single file may beat or miss the
//! average. Two ffprobe-derived refinements then adjust individual files: the
//! per-file **maxrate ceiling** (`maxrate · duration`) bounds the fattest clips,
//! and a **frame-rate factor** halves clips whose source exceeds the fps cap.
//!
//! Finally, [`calibration_factor`] nudges the seeded compaction toward what *this*
//! machine + library + settings actually produced (read straight from the
//! manifest), so the estimate self-improves as a run progresses.

use crate::config::EncodeConfig;
use crate::manifest::{Manifest, Status};

/// Blended output/input byte ratio at cq30, from the reference run
/// (187.6 GB → 28.9 GB ≈ 6.5×, so ≈ 0.154). This is a *library average*, not a
/// single clip: fat low-complexity clips do better, busy clips worse.
const BASE_COMPACTION_CQ30: f64 = 0.154;

/// Output-size multiplier per +1 in `cq`. Derived from the single-clip cq sweep
/// (cq30/32/34 → 154/120/93 MB, a consistent ≈0.777× per +2 cq, i.e. ≈0.881 per
/// +1). Lower cq = higher quality = larger output.
const CQ_STEP: f64 = 0.881;

/// Don't trust empirical calibration until at least this many files are done —
/// with too few samples (and largest-first ordering) the realized ratio is noisy.
const MIN_CALIB_SAMPLES: usize = 5;

/// Clamp the calibration correction to a sane band so an early outlier (or a
/// pathological manifest) can't blow the estimate up or collapse it to nothing.
const CALIB_MIN: f64 = 0.3;
const CALIB_MAX: f64 = 3.0;

/// The cheap, per-file facts the estimate needs: duration + frame rate (from
/// ffprobe) and the source size (from the scan).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeFacts {
    pub duration_secs: f64,
    pub fps: f64,
    pub src_bytes: u64,
}

/// The seeded blended compaction (output/input byte ratio) the model assumes for
/// a given `cq`, absent any empirical calibration. `cq30 → ≈0.154`, decreasing as
/// `cq` rises.
pub fn quality_compaction(cq: u32) -> f64 {
    BASE_COMPACTION_CQ30 * CQ_STEP.powf(cq as f64 - 30.0)
}

/// Parse an ffmpeg bitrate string (`"12M"`, `"8000k"`, `"5000000"`) into bits per
/// second. Suffixes are **decimal** (ffmpeg convention: `k = 1e3`, `M = 1e6`).
/// Returns `None` for empty / zero / unparseable input, which callers treat as
/// "no ceiling".
pub fn parse_bitrate(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        // Suffix chars are ASCII, so trimming the last byte stays on a boundary.
        Some('k' | 'K') => (&s[..s.len() - 1], 1_000.0),
        Some('m' | 'M') => (&s[..s.len() - 1], 1_000_000.0),
        Some('g' | 'G') => (&s[..s.len() - 1], 1_000_000_000.0),
        _ => (s, 1.0),
    };
    let value: f64 = num.trim().parse().ok()?;
    let bits = (value * mult) as u64;
    (bits > 0).then_some(bits)
}

/// Estimate the compressed size of one file in bytes.
///
/// `calib` is the empirical correction from [`calibration_factor`] (`1.0` when no
/// history is available yet). The result is `min(analytic, maxrate ceiling,
/// source)` — it never predicts an output larger than the source.
pub fn estimate_output_bytes(facts: &ProbeFacts, enc: &EncodeConfig, calib: f64) -> u64 {
    let src = facts.src_bytes as f64;
    // No duration or no size to work from → assume the file passes through unchanged.
    if src <= 0.0 || facts.duration_secs <= 0.0 {
        return facts.src_bytes;
    }

    let compaction = quality_compaction(enc.cq) * calib.max(0.0);

    // Frame-rate factor: a clip dropped from e.g. 60→30 fps needs ~half the
    // bitrate for the same quality. Applied only when the source exceeds the cap
    // (matches `encode::build_vf`); the reference library is mostly 30 fps, so
    // this rarely triggers.
    let fps_factor = if enc.fps_cap > 0 && facts.fps > enc.fps_cap as f64 + 0.1 {
        enc.fps_cap as f64 / facts.fps
    } else {
        1.0
    };

    let analytic = src * compaction * fps_factor;

    // Per-file ceiling from the bitrate cap — this is what the ffprobe duration
    // unlocks. Output bitrate can't exceed `maxrate`, so output bytes can't
    // exceed `maxrate · duration / 8`.
    let capped = match parse_bitrate(&enc.maxrate) {
        Some(bps) => analytic.min(bps as f64 / 8.0 * facts.duration_secs),
        None => analytic,
    };

    capped.min(src).round() as u64
}

/// Sum a library's estimated output. Each item is `(exact_output, facts)`:
/// already-compressed files pass their **real** size as `Some(bytes)` (used
/// verbatim); pending files pass `None` and are estimated from `facts`.
pub fn aggregate<I>(items: I, enc: &EncodeConfig, calib: f64) -> u64
where
    I: IntoIterator<Item = (Option<u64>, ProbeFacts)>,
{
    items
        .into_iter()
        .map(|(exact, facts)| exact.unwrap_or_else(|| estimate_output_bytes(&facts, enc, calib)))
        .sum()
}

/// Derive an empirical correction factor from already-compressed manifest
/// entries: how the realized blended compaction compares to what the seeded model
/// assumes. Returns `None` until [`MIN_CALIB_SAMPLES`] files are done; otherwise a
/// factor clamped to `[CALIB_MIN, CALIB_MAX]` that callers multiply into
/// [`estimate_output_bytes`].
///
/// Note: jobs run largest-first, so the earliest samples are the fattest clips
/// (which compress best); the factor is biased low early and converges as more
/// files of mixed bitrate complete.
pub fn calibration_factor(manifest: &Manifest, cq: u32) -> Option<f64> {
    let mut in_sum: u64 = 0;
    let mut out_sum: u64 = 0;
    let mut samples = 0usize;

    for entry in manifest.entries.values() {
        if matches!(entry.status, Status::Compressed | Status::Uploaded)
            && let Some(out) = entry.output_bytes
            && entry.source_bytes > 0
        {
            in_sum += entry.source_bytes;
            out_sum += out;
            samples += 1;
        }
    }

    if samples < MIN_CALIB_SAMPLES || in_sum == 0 {
        return None;
    }

    let realized = out_sum as f64 / in_sum as f64;
    let seeded = quality_compaction(cq);
    if seeded <= 0.0 {
        return None;
    }
    Some((realized / seeded).clamp(CALIB_MIN, CALIB_MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Entry;

    fn enc(cq: u32) -> EncodeConfig {
        EncodeConfig {
            codec: "hevc".into(),
            cq,
            maxrate: "12M".into(),
            fps_cap: 30,
            audio: "copy".into(),
            jobs: 1,
            scale: None,
        }
    }

    // ── parse_bitrate ────────────────────────────────────────────────────────

    #[test]
    fn parse_bitrate_handles_suffixes_and_plain_numbers() {
        assert_eq!(parse_bitrate("12M"), Some(12_000_000));
        assert_eq!(parse_bitrate("8M"), Some(8_000_000));
        assert_eq!(parse_bitrate("128k"), Some(128_000));
        assert_eq!(parse_bitrate("5000000"), Some(5_000_000));
        assert_eq!(parse_bitrate(" 16m "), Some(16_000_000)); // trimmed, lowercase
    }

    #[test]
    fn parse_bitrate_rejects_zero_and_garbage() {
        assert_eq!(parse_bitrate(""), None);
        assert_eq!(parse_bitrate("0"), None);
        assert_eq!(parse_bitrate("0M"), None);
        assert_eq!(parse_bitrate("garbage"), None);
    }

    // ── quality_compaction ───────────────────────────────────────────────────

    #[test]
    fn quality_compaction_anchored_at_cq30() {
        assert!((quality_compaction(30) - 0.154).abs() < 1e-9);
    }

    #[test]
    fn quality_compaction_decreases_as_cq_rises() {
        // Higher cq = smaller output = lower compaction ratio.
        assert!(quality_compaction(28) > quality_compaction(30));
        assert!(quality_compaction(30) > quality_compaction(32));
        assert!(quality_compaction(32) > quality_compaction(34));
        // cq28 ≈ 0.154 / 0.881^2 ≈ 0.198 — reproduces the GUI's "Higher ≈ 5×" guess.
        assert!((quality_compaction(28) - 0.198).abs() < 0.005);
    }

    // ── estimate_output_bytes ────────────────────────────────────────────────

    #[test]
    fn estimate_matches_blended_model_on_reference_clip() {
        // The Alan Wake sample: 1304 MiB, 180 s, 30 fps. The blended model predicts
        // the library-average compaction (≈0.154 → ≈201 MB), deliberately *not* the
        // 154 MB this below-average-complexity clip actually hit — single clips beat
        // or miss the average; the per-library *total* is what's accurate.
        let src = 1304 * 1024 * 1024;
        let facts = ProbeFacts {
            duration_secs: 180.0,
            fps: 30.0,
            src_bytes: src,
        };
        let est = estimate_output_bytes(&facts, &enc(30), 1.0);
        let expected = (src as f64 * 0.154) as u64;
        assert!(
            est.abs_diff(expected) < expected / 50, // within 2%
            "est {est} vs expected {expected}"
        );
        assert!(est < src); // always shrinks
    }

    #[test]
    fn maxrate_ceiling_binds_on_very_fat_clips() {
        // A 90 Mbps, 100 s clip at cq30: analytic = 0.154 × src, but the 12M cap
        // (→ 150 MB) is the lower, binding bound.
        let src = (90_000_000.0 / 8.0 * 100.0) as u64; // 1.125 GB
        let facts = ProbeFacts {
            duration_secs: 100.0,
            fps: 30.0,
            src_bytes: src,
        };
        let cap_bytes = (12_000_000.0 / 8.0 * 100.0) as u64; // 150 MB
        let est = estimate_output_bytes(&facts, &enc(30), 1.0);
        assert_eq!(est, cap_bytes);
    }

    #[test]
    fn fps_cap_halves_60fps_sources() {
        // Small clip so the maxrate cap never binds; only the fps factor differs.
        let src = 200_000_000;
        let at30 = estimate_output_bytes(
            &ProbeFacts {
                duration_secs: 100.0,
                fps: 30.0,
                src_bytes: src,
            },
            &enc(30),
            1.0,
        );
        let at60 = estimate_output_bytes(
            &ProbeFacts {
                duration_secs: 100.0,
                fps: 60.0,
                src_bytes: src,
            },
            &enc(30),
            1.0,
        );
        // 60 fps → halved to 30 → ~half the bytes of the same-size 30 fps clip.
        assert!((at60 as f64 - at30 as f64 / 2.0).abs() < at30 as f64 * 0.02);
    }

    #[test]
    fn never_predicts_growth_even_with_large_calibration() {
        // Long duration so the 12M ceiling (→ 450 MB) doesn't bind first; this
        // isolates the min(src) guard. calib=10 pushes analytic (308 MB) above the
        // source (200 MB), so the result must clamp back to the source size.
        let facts = ProbeFacts {
            duration_secs: 300.0,
            fps: 30.0,
            src_bytes: 200_000_000,
        };
        let est = estimate_output_bytes(&facts, &enc(30), 10.0);
        assert_eq!(est, facts.src_bytes);
    }

    #[test]
    fn missing_duration_or_size_passes_through() {
        let no_dur = ProbeFacts {
            duration_secs: 0.0,
            fps: 30.0,
            src_bytes: 123,
        };
        assert_eq!(estimate_output_bytes(&no_dur, &enc(30), 1.0), 123);
        let no_bytes = ProbeFacts {
            duration_secs: 100.0,
            fps: 30.0,
            src_bytes: 0,
        };
        assert_eq!(estimate_output_bytes(&no_bytes, &enc(30), 1.0), 0);
    }

    // ── aggregate ────────────────────────────────────────────────────────────

    #[test]
    fn aggregate_uses_exact_for_done_and_estimate_for_pending() {
        let facts = ProbeFacts {
            duration_secs: 100.0,
            fps: 30.0,
            src_bytes: 200_000_000,
        };
        let est_one = estimate_output_bytes(&facts, &enc(30), 1.0);
        let total = aggregate(
            [
                (Some(50_000_000), facts), // done → exact, facts ignored
                (None, facts),             // pending → estimated
            ],
            &enc(30),
            1.0,
        );
        assert_eq!(total, 50_000_000 + est_one);
    }

    #[test]
    fn aggregate_of_empty_is_zero() {
        assert_eq!(aggregate(std::iter::empty(), &enc(30), 1.0), 0);
    }

    // ── calibration_factor ───────────────────────────────────────────────────

    fn done(src: u64, out: u64) -> Entry {
        Entry {
            status: Status::Compressed,
            source_bytes: src,
            output_bytes: Some(out),
        }
    }

    fn manifest_of(entries: &[(&str, Entry)]) -> Manifest {
        let mut m = Manifest::default();
        for (k, e) in entries {
            m.entries.insert((*k).to_string(), e.clone());
        }
        m
    }

    #[test]
    fn calibration_needs_minimum_samples() {
        assert_eq!(calibration_factor(&Manifest::default(), 30), None);
        let few = manifest_of(&[("a", done(1000, 200)), ("b", done(1000, 200))]);
        assert_eq!(calibration_factor(&few, 30), None); // < MIN_CALIB_SAMPLES
    }

    #[test]
    fn calibration_returns_realized_over_seeded() {
        // 5 files, each realized 0.20 vs seeded 0.154 → factor ≈ 1.30.
        let m = manifest_of(&[
            ("a", done(1000, 200)),
            ("b", done(1000, 200)),
            ("c", done(1000, 200)),
            ("d", done(1000, 200)),
            ("e", done(1000, 200)),
        ]);
        let f = calibration_factor(&m, 30).unwrap();
        assert!((f - 0.20 / 0.154).abs() < 1e-6);
    }

    #[test]
    fn calibration_clamps_extremes() {
        // Realized 0.01 → 0.01/0.154 ≈ 0.065, clamped up to CALIB_MIN.
        let tiny = manifest_of(&[
            ("a", done(1000, 10)),
            ("b", done(1000, 10)),
            ("c", done(1000, 10)),
            ("d", done(1000, 10)),
            ("e", done(1000, 10)),
        ]);
        assert_eq!(calibration_factor(&tiny, 30), Some(CALIB_MIN));
        // Realized 0.90 → 0.90/0.154 ≈ 5.84, clamped down to CALIB_MAX.
        let huge = manifest_of(&[
            ("a", done(1000, 900)),
            ("b", done(1000, 900)),
            ("c", done(1000, 900)),
            ("d", done(1000, 900)),
            ("e", done(1000, 900)),
        ]);
        assert_eq!(calibration_factor(&huge, 30), Some(CALIB_MAX));
    }

    #[test]
    fn calibration_counts_uploaded_and_ignores_pending() {
        let mut m = manifest_of(&[
            ("a", done(1000, 200)),
            ("b", done(1000, 200)),
            ("c", done(1000, 200)),
            ("d", done(1000, 200)),
        ]);
        // A legacy "uploaded" entry should still count toward calibration.
        let mut up = done(1000, 200);
        up.status = Status::Uploaded;
        m.entries.insert("e".into(), up);
        // A pending entry (no output_bytes) must be ignored, not counted as 0.
        m.entries.insert(
            "f".into(),
            Entry {
                status: Status::Pending,
                source_bytes: 1000,
                output_bytes: None,
            },
        );
        let f = calibration_factor(&m, 30).unwrap();
        assert!((f - 0.20 / 0.154).abs() < 1e-6); // 5 counted, pending excluded
    }
}
