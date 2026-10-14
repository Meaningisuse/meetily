#!/usr/bin/env python3
"""Run the isolated sherpa-onnx diarization CLI and score it against RTTM."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import signal
import shutil
import subprocess
import sys
import time
import wave
from pathlib import Path
from typing import Iterable

TURN = re.compile(r"^\s*(\d+(?:\.\d+)?)\s*--\s*(\d+(?:\.\d+)?)\s+speaker[_-]?(\S+)\s*$", re.I)
FRAME_SECONDS = 0.01


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_rttm(path: Path) -> list[tuple[float, float, str]]:
    turns = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        fields = line.split()
        if not fields:
            continue
        if len(fields) < 8 or fields[0] != "SPEAKER":
            raise ValueError(f"invalid RTTM row at {path}:{line_number}")
        start, duration = float(fields[3]), float(fields[4])
        if start < 0 or duration <= 0:
            raise ValueError(f"invalid RTTM interval at {path}:{line_number}")
        turns.append((start, start + duration, fields[7]))
    if not turns:
        raise ValueError("reference RTTM contains no speaker turns")
    return turns


def active(turns: Iterable[tuple[float, float, str]], instant: float) -> set[str]:
    return {speaker for start, end, speaker in turns if start <= instant < end}


def collar_frames(reference: list[tuple[float, float, str]], total: int, collar: float) -> set[int]:
    boundaries = [point for start, end, _ in reference for point in (start, end)]
    return {
        frame
        for frame in range(total)
        if any(abs((frame + 0.5) * FRAME_SECONDS - edge) <= collar for edge in boundaries)
    }


def map_labels(reference: list[tuple[float, float, str]], hypothesis: list[tuple[float, float, str]], total: int) -> dict[str, str]:
    ref_labels = sorted({speaker for _, _, speaker in reference})
    hyp_labels = sorted({speaker for _, _, speaker in hypothesis})
    if len(ref_labels) > 8:
        raise ValueError("DER scorer supports at most eight reference speakers per recording")
    ref_frames = [active(reference, (i + 0.5) * FRAME_SECONDS) for i in range(total)]
    hyp_frames = [active(hypothesis, (i + 0.5) * FRAME_SECONDS) for i in range(total)]
    # Dynamic programming over the (at most eight) reference speakers gives an
    # exact one-to-one maximum-overlap assignment while allowing any number of
    # hypothesis speakers. Unmatched hypothesis labels remain false alarms.
    overlap = {label: {ref: 0 for ref in ref_labels} for label in hyp_labels}
    for ref, hyp in zip(ref_frames, hyp_frames):
        for label in hyp:
            for speaker in ref:
                overlap[label][speaker] += 1

    states: dict[int, tuple[int, tuple[str | None, ...]]] = {0: (0, ())}
    for label in hyp_labels:
        next_states: dict[int, tuple[int, tuple[str | None, ...]]] = {}
        for mask, (score, assignment) in states.items():
            previous = next_states.get(mask)
            skipped = (score, assignment + (None,))
            if previous is None or skipped[0] > previous[0]:
                next_states[mask] = skipped
            for index, speaker in enumerate(ref_labels):
                bit = 1 << index
                if mask & bit:
                    continue
                updated_mask = mask | bit
                matched = (score + overlap[label][speaker], assignment + (speaker,))
                previous = next_states.get(updated_mask)
                if previous is None or matched[0] > previous[0]:
                    next_states[updated_mask] = matched
        states = next_states

    _, assignment = max(states.values(), key=lambda state: state[0])
    best = {
        label: speaker if speaker is not None else f"__miss_{index}"
        for index, (label, speaker) in enumerate(zip(hyp_labels, assignment))
    }
    return best


def der(reference: list[tuple[float, float, str]], hypothesis: list[tuple[float, float, str]], duration: float, collar: float, include_overlap: bool) -> float:
    total = int(duration / FRAME_SECONDS)
    ignored = collar_frames(reference, total, collar)
    mapping = map_labels(reference, hypothesis, total)
    errors = reference_speech = 0
    for frame in range(total):
        if frame in ignored:
            continue
        at = (frame + 0.5) * FRAME_SECONDS
        ref = active(reference, at)
        if not include_overlap and len(ref) > 1:
            continue
        hyp = {mapping[label] for label in active(hypothesis, at)}
        hits = len(ref & hyp)
        reference_speech += len(ref)
        errors += max(0, len(ref) - len(hyp)) + max(0, len(hyp) - len(ref)) + min(len(ref), len(hyp)) - hits
    if not reference_speech:
        raise ValueError("RTTM has no reference speech outside the selected collar/overlap policy")
    return errors / reference_speech


def parse_turns(output: str) -> list[tuple[float, float, str]]:
    turns = []
    for line in output.splitlines():
        match = TURN.match(line)
        if match:
            start, end = float(match.group(1)), float(match.group(2))
            if end > start:
                turns.append((start, end, match.group(3)))
    if not turns:
        raise ValueError("sherpa output contained no parseable speaker turns")
    return turns


def peak_rss_mb(pid: int, stop: subprocess.Popen[str], samples: list[int]) -> None:
    while stop.poll() is None:
        try:
            value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()
            if value.isdigit():
                samples.append(int(value))
        except (OSError, subprocess.CalledProcessError):
            pass
        time.sleep(0.1)


def run(args: argparse.Namespace) -> dict:
    audio, segmentation, embedding, reference_path = map(Path, (args.audio, args.segmentation_model, args.embedding_model, args.reference_rttm))
    binary = shutil.which(args.binary) or args.binary
    for required in (audio, segmentation, embedding, reference_path, Path(binary)):
        if not required.is_file():
            raise FileNotFoundError(f"required probe input is missing: {required}")
    with wave.open(str(audio), "rb") as stream:
        duration = stream.getnframes() / stream.getframerate()
    if duration <= 0:
        raise ValueError("audio file contains no frames")
    reference = read_rttm(reference_path)
    cmd = [binary, f"--segmentation.pyannote-model={segmentation}", f"--embedding.model={embedding}"]
    if args.expected_speakers:
        cmd.append(f"--clustering.num-clusters={args.expected_speakers}")
    else:
        cmd.append(f"--clustering.cluster-threshold={args.cluster_threshold}")
    cmd.append(str(audio))

    rss_samples: list[int] = []
    started = time.monotonic()
    process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    monitor = __import__("threading").Thread(target=peak_rss_mb, args=(process.pid, process, rss_samples), daemon=True)
    monitor.start()
    try:
        stdout, stderr = process.communicate(timeout=args.timeout_seconds)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        process.communicate()
        raise TimeoutError(f"diarization exceeded {args.timeout_seconds} seconds")
    elapsed = time.monotonic() - started
    monitor.join(timeout=0.5)
    if process.returncode != 0:
        raise RuntimeError(f"sherpa exited with status {process.returncode}: {stderr[-2000:]}")
    hypothesis = parse_turns(stdout + "\n" + stderr)
    frames = int(duration / FRAME_SECONDS)
    report = {
        "schema_version": 1,
        "sample_id": args.sample_id,
        "hardware": {"os": platform.platform(), "architecture": platform.machine(), "cpu_count": os.cpu_count(), "processor": platform.processor() or "unknown"},
        "model_hash": {"segmentation_sha256": sha256(segmentation), "embedding_sha256": sha256(embedding)},
        "runtime_version": args.runtime_version,
        "audio_minutes": duration / 60,
        "der": der(reference, hypothesis, duration, 0.25, False),
        "overlap_der": der(reference, hypothesis, duration, 0.25, True),
        "rtf": elapsed / duration,
        "peak_rss_mb": max(rss_samples, default=0) / 1024 if rss_samples else None,
        "cancel_latency_ms": None,
        "cancel_probe_mode": "not measured; CLI process termination is not equivalent to application cancellation",
        "license": {"runtime": args.runtime_license, "segmentation_weights": args.segmentation_license, "embedding_weights": args.embedding_license},
        "download_conditions": args.download_conditions,
        "packaging_bytes": segmentation.stat().st_size + embedding.stat().st_size,
        "sample_sha256": sha256(audio),
        "turn_count": len(hypothesis),
        "reference_turn_count": len(reference),
        "frame_count": frames,
        "quality_gate": "PASS" if der(reference, hypothesis, duration, 0.25, False) <= 0.20 else "FAIL",
        "performance_gate": "PASS" if rss_samples and elapsed / duration <= 0.5 and max(rss_samples) <= 2 * 1024 * 1024 else "FAIL",
    }
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="sherpa-onnx-offline-speaker-diarization")
    parser.add_argument("--audio", required=True, help="authorized, anonymized WAV sample")
    parser.add_argument("--reference-rttm", required=True, help="human-verified RTTM for this sample")
    parser.add_argument("--sample-id", required=True, help="anonymous ID, never a meeting title/path")
    parser.add_argument("--segmentation-model", required=True)
    parser.add_argument("--embedding-model", required=True)
    parser.add_argument("--expected-speakers", type=int)
    parser.add_argument("--cluster-threshold", type=float, default=0.5)
    parser.add_argument("--timeout-seconds", type=int, default=7200)
    parser.add_argument("--runtime-version", default="unknown")
    parser.add_argument("--runtime-license", default="unverified")
    parser.add_argument("--segmentation-license", default="unverified")
    parser.add_argument("--embedding-license", default="unverified")
    parser.add_argument("--download-conditions", default="unverified")
    parser.add_argument("--output", type=Path, required=True, help="private report path; do not commit sample paths")
    args = parser.parse_args()
    try:
        result = run(args)
    except (OSError, ValueError, RuntimeError, TimeoutError, wave.Error) as error:
        print(f"probe failed: {error}", file=sys.stderr)
        return 2
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
