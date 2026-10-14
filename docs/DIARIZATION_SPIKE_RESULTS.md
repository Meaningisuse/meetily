# P4 D1 diarization feasibility gate

**Decision: NO-GO for D3–D5.** Continue D2 timing work independently. Do not add a diarization runtime/model to the app until the listed gates are met. This is a feasibility decision from available evidence, not a model-quality result.

## Probe host

- OS: macOS 27.0.1, arm64.
- Hardware: MacBook Pro `Mac17,2`, Apple M5, 10 CPU cores, 24 GB unified memory, 10-core Apple GPU, Metal supported.
- Meetily's local `models/parakeet` directory is empty; no sherpa-onnx diarization CLI, segmentation model, or speaker embedding model was found in the app model directory.
- Probe sample set in this checkout: 0 authorized meeting samples / 0 annotated minutes / 0 RTTM files. The repository contains no human-labeled Chinese diarization holdout. The existing `jfk.wav` fixture is not a multi-speaker Chinese meeting and has no RTTM annotation.

## Candidate evidence

The sherpa-onnx official documentation exposes offline speaker diarization APIs and identifies separate segmentation and embedding model releases. Its Rust example pairs `sherpa-onnx-pyannote-segmentation-3-0` with `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx`, and demonstrates Chinese audio. This establishes a plausible probe path, not measured quality, app integration, or package readiness: [sherpa diarization docs](https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html), [official Rust example](https://github.com/k2-fsa/sherpa-onnx/blob/master/rust-api-examples/examples/offline_speaker_diarization.rs).

The individual converted ONNX release artifacts' redistribution terms still need written source-level confirmation. A September 2026 sherpa-onnx discussion asks about precisely these artifacts; the maintainer directs the question to the original authors without confirming artifact redistribution terms: [model license discussion](https://github.com/k2-fsa/sherpa-onnx/discussions/3939). The alternative `pyannote/speaker-diarization-community-1` model card declares CC BY 4.0 and gates access on accepting contact-information conditions: [model card](https://huggingface.co/pyannote/speaker-diarization-community-1). Neither is treated here as cleared for redistribution in the Meetily app bundle.

## Results and gates

| Measure | Required | Result |
| --- | --- | --- |
| Authorized evaluation set | ≥10 samples, ≥90 minutes, 20% holdout | Not available; gate blocked |
| Main DER | ≤20%, 250 ms collar, overlap excluded | Not measured |
| Overlap DER | Report separately | Not measured |
| Long recording | 120 minutes | Not available |
| RTF | ≤0.5 | Not measured |
| Peak RSS | ≤2 GiB | Not measured |
| App cancellation | Response ≤3 seconds | Not measured; candidate CLI has no verified cooperative cancellation contract |
| Artifact packaging | Offline app-bundle size and load | Not measured |
| Model-weight licensing | Confirm each artifact and notices | Not cleared |
| Transcription timing | Whisper/Parakeet true-time and Chinese-tail regression | D2 remains to be measured |

The Go gate therefore fails on missing authorized labels, unavailable runtime/model inputs, unresolved model artifact terms, and missing cancellation/packaging evidence. Benchmark numbers from model cards or upstream demo WAVs cannot replace this project's held-out Chinese meeting evaluation.

## Reproducibility artifacts

`scripts/diarization-probe/run_probe.py` runs the external sherpa CLI against a WAV plus human-verified RTTM and produces a private JSON report with hardware, model hashes, version, DER/overlap DER, RTF, peak RSS, license/access notes, and model bytes. It intentionally does not download data or models. Private audio, RTTM, model weights, and reports are ignored by Git; only anonymous aggregate evidence may be committed.

The probe was smoke-tested with a generated 10-second silent WAV, synthetic RTTM, placeholder model files, and a fake CLI that returns matching turns. This checks argument wiring, report serialization, and DER plumbing only; it is not diarization, model, performance, cancellation, or quality evidence.

To change this decision, prepare and authorize the minimum set described in [`frontend/src-tauri/tests/fixtures/diarization/README.md`](../frontend/src-tauri/tests/fixtures/diarization/README.md), make model/runtime versions reproducible, verify each weight's terms and packaging rights, and run the probe on the holdout plus a separate 120-minute load sample. Until then, D3–D5 remain paused by the plan's explicit gate.

## D2 timing status (2026-10-14)

**Status: foundation implemented; provider-level precise timing remains unverified and is not enabled.** The new `audio::time_map` helper maps half-open intervals from packed 16 kHz samples to original source sample spans. It splits output at source discontinuities and rejects unmapped or overlapping packed ranges. Absolute sample-rate conversion uses one rational operation to avoid per-chunk rounding drift.

The focused Rust tests pass for a VAD gap (packed 1–2 s maps to source 10–11 s), an interval crossing that gap (returned as two spans), 44.1/48 kHz conversion over 120 minutes, and invalid/unmapped ranges. These tests verify mapping arithmetic only.

Existing transcription providers still return plain text. Whisper currently disables its no-timestamp optimization because that path has previously skipped valid chunks; Parakeet exposes only text. Import and retranscription retain VAD segment start/end positions, which are approximate segment bounds rather than provider word timings. No authorized Chinese regression set or local model is available to prove a timestamp-enabled Whisper path preserves Chinese tail text or validate timing on real audio. This change adds no provider timestamp behavior, transcript precision field, schema migration, or exact-timing claim. D2 remains incomplete pending a real-model regression fixture and end-to-end timing readback.
