# Isolated diarization feasibility probe

This script is a one-off engineering probe. It is not part of Meetily's app runtime and does not restore the retired Python backend. It expects an already-built `sherpa-onnx-offline-speaker-diarization` CLI, local model files, an authorized WAV, and a human-verified RTTM. It does not download audio/models or print the meeting path into the JSON report.

Run it from a separate virtual environment or host with the sherpa CLI installed:

```sh
python3 scripts/diarization-probe/run_probe.py \
  --binary /absolute/path/to/sherpa-onnx-offline-speaker-diarization \
  --audio /private/probe/sample-01.wav \
  --reference-rttm /private/probe/sample-01.rttm \
  --sample-id zh-meeting-01 \
  --segmentation-model /private/models/segmentation/model.onnx \
  --embedding-model /private/models/3dspeaker_zh.onnx \
  --expected-speakers 4 \
  --runtime-version '<exact version>' \
  --runtime-license '<verified license>' \
  --segmentation-license '<verified artifact license>' \
  --embedding-license '<verified artifact license>' \
  --download-conditions '<reviewed terms>' \
  --output /private/probe/results/zh-meeting-01.json
```

Provide mono 16 kHz WAV inputs. Each RTTM row must use the standard `SPEAKER` format. The primary DER uses a 250 ms reference collar and excludes reference overlap; `overlap_der` includes overlap frames. Predicted speaker labels are mapped to reference labels by maximum overlap before scoring. Use at least 10 independently annotated samples totaling 90 minutes, preserve a 20% holdout, and run an additional 120-minute file for memory/RTF. Do not publish private audio or RTTM; record only anonymous IDs and aggregate metrics in the repository.

The CLI process has no verified cooperative cancellation contract. This script leaves `cancel_latency_ms` unset rather than treating an external kill as safe app cancellation. Benchmark app cancellation separately after a runtime candidate exists.

The probe exits with code 2 on missing inputs/runtime or malformed output; successful execution writes a private JSON report. Quality/performance fields describe only the supplied sample and must not be generalized from a small or non-held-out set.
