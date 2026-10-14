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

### Public Mandarin corpus candidates checked (2026-10-14)

Two potential labeled sources exist, so the evidence is no longer “no public Mandarin meeting corpus found”; neither has been downloaded or assembled into the project holdout:

- [AISHELL-4 on OpenSLR](https://www.openslr.org/111) is listed as CC BY-SA 4.0 and described as 120 hours across 211 recorded meetings with 4–8 speakers, overlap/noise/quick turns, transcriptions, and speaker activity labels. Its smallest listed training archive is still 14 GB. The AISHELL baseline repository's Apache-2.0 applies to its code; it must not be mistaken for the OpenSLR corpus license.
- [AliMeeting on OpenSLR](https://www.openslr.org/119) is also listed CC BY-SA 4.0 and described as 118.75 hours across 240 meetings with 2–4 speakers. The listed Eval archive is 3.42 GB and includes far-field and headset audio.

These corpora could supply Mandarin multi-speaker evaluation material under their published data license with attribution and ShareAlike obligations. They do not by themselves cover a real one-speaker meeting, code-switching, or all device/compression conditions, and a held-out subset must avoid overlap with any model's training data. No archive has been fetched; no claim is made that a dataset is locally ready. Separately, the official [Rev Reverb model license](https://huggingface.co/Revai/reverb-diarization-v1/blob/main/LICENSE) limits use to testing/research/personal/evaluation in Non-Production Environments and expressly excludes internal employee usage in an employer's commercial business activities; this candidate is unsuitable for Meetily's stated internal business use. The [pyannote segmentation weights](https://huggingface.co/pyannote/segmentation-3.0/blob/main/LICENSE) declare MIT, but that only clears that component, not a complete diarization stack's embedding weights or converted bundle artifacts.

## Results and gates

| Measure | Required | Result |
| --- | --- | --- |
| Authorized evaluation set | ≥10 samples, ≥90 minutes, 20% holdout | Public corpus candidates identified; local stratified set not assembled |
| Main DER | ≤20%, 250 ms collar, overlap excluded | Not measured |
| Overlap DER | Report separately | Not measured |
| Long recording | 120 minutes | Not available |
| RTF | ≤0.5 | Not measured |
| Peak RSS | ≤2 GiB | Not measured |
| App cancellation | Response ≤3 seconds | Not measured; candidate CLI has no verified cooperative cancellation contract |
| Artifact packaging | Offline app-bundle size and load | Not measured |
| Model-weight licensing | Confirm each artifact and notices | Not cleared |
| Transcription timing | Whisper/Parakeet true-time and Chinese-tail regression | D2 remains to be measured |

The Go gate therefore remains NO-GO: no local held-out set is assembled; the proposed sherpa/3D-Speaker release weights still lack written artifact-level redistribution confirmation; and runtime, cancellation, packaging, and quality evidence are missing. Benchmark numbers from model cards or upstream demo WAVs cannot replace this project's held-out Chinese meeting evaluation.

## Reproducibility artifacts

`scripts/diarization-probe/run_probe.py` runs the external sherpa CLI against a WAV plus human-verified RTTM and produces a private JSON report with hardware, model hashes, version, DER/overlap DER, RTF, peak RSS, license/access notes, and model bytes. It intentionally does not download data or models. Private audio, RTTM, model weights, and reports are ignored by Git; only anonymous aggregate evidence may be committed.

The probe was smoke-tested with a generated 10-second silent WAV, synthetic RTTM, placeholder model files, and a fake CLI that returns matching turns. This checks argument wiring, report serialization, and DER plumbing only; it is not diarization, model, performance, cancellation, or quality evidence.

To change this decision, prepare and authorize the minimum set described in [`frontend/src-tauri/tests/fixtures/diarization/README.md`](../frontend/src-tauri/tests/fixtures/diarization/README.md), make model/runtime versions reproducible, verify each weight's terms and packaging rights, and run the probe on the holdout plus a separate 120-minute load sample. Until then, D3–D5 remain paused by the plan's explicit gate.

## D2 timing status (2026-10-14)

**Status: mapping and precision-persistence foundations implemented; provider-level precise timing remains unverified and is not enabled.** The `audio::time_map` helper maps half-open intervals from packed 16 kHz samples to original source sample spans. It splits output at source discontinuities and rejects unmapped or overlapping packed ranges. Absolute sample-rate conversion uses one rational operation to avoid per-chunk rounding drift.

The focused Rust tests pass for a VAD gap (packed 1–2 s maps to source 10–11 s), an interval crossing that gap (returned as two spans), 44.1/48 kHz conversion over 120 minutes, and invalid/unmapped ranges. These tests verify mapping arithmetic only.

Whisper and Parakeet still return plain text, so their provider result carries `unavailable` with no timed segments. Whisper currently disables its no-timestamp optimization because that path has previously skipped valid chunks. Import, retranscription, and recording saves persist VAD/chunk bounds as `approximate`; rows without valid bounds remain `unavailable`. Existing rows are backfilled by the new migration using the same rule, and meeting documents expose the precision in schema version 2. Tests cover unavailable provider timing, legacy-data backfill, recording persistence, and API/document readback. No authorized Chinese regression set or local model is available to prove a timestamp-enabled Whisper path preserves Chinese tail text or validate timing on real audio. Provider-supplied timed segments and three-entry playback verification remain incomplete, so D2 is not complete and no exact-timing claim is made.
