#!/usr/bin/env python3
"""Prepare private anonymous mono WAV/RTTM samples from AliMeeting Test."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tarfile
import wave
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

from textgrid_to_rttm import convert_textgrid_to_rttm, read_speaker_turns


MEETING = re.compile(r"(?:^|_)M(?P<id>\d+)(?:_|\.|$)", re.I)
TEXTGRID_LABEL = "TextGrid contains no labeled speaker turns"


@dataclass(frozen=True)
class Sample:
    meeting_id: str
    audio_member: str
    textgrid_member: str
    speaker_count: int
    reference_turn_count: int
    duration_seconds: float


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def meeting_id(name: str) -> str:
    match = MEETING.search(Path(name).name)
    if not match:
        raise ValueError("AliMeeting member name has no meeting identifier")
    return match.group("id")


def inspect_archive(archive: Path) -> list[Sample]:
    grids: dict[str, tuple[str, int, int]] = {}
    wavs: dict[str, tuple[str, int, int, int]] = {}
    with tarfile.open(archive, "r:gz") as tar:
        for member in tar:
            if not member.isfile():
                continue
            suffix = Path(member.name).suffix.lower()
            stem = Path(member.name).stem
            if suffix == ".textgrid":
                source = tar.extractfile(member)
                if source is None:
                    raise ValueError("could not read TextGrid member")
                content = source.read().decode("utf-8", errors="strict")
                try:
                    turns = read_speaker_turns(content)
                except ValueError as error:
                    if str(error) == TEXTGRID_LABEL:
                        continue
                    raise
                speaker_count = len({speaker for _, _, speaker in turns})
                if not 2 <= speaker_count <= 8:
                    raise ValueError("Test reference must contain between two and eight speakers")
                identifier = meeting_id(member.name)
                if identifier in grids:
                    raise ValueError("duplicate speaker-labeled TextGrid meeting")
                grids[identifier] = (member.name, speaker_count, len(turns))
            elif suffix == ".wav":
                source = tar.extractfile(member)
                if source is None:
                    raise ValueError("could not read WAV member")
                with wave.open(source, "rb") as audio:
                    channels = audio.getnchannels()
                    sample_rate = audio.getframerate()
                    if channels == 8 and sample_rate == 16000:
                        identifier = meeting_id(member.name)
                        if identifier in wavs:
                            raise ValueError("duplicate far-field WAV meeting")
                        wavs[identifier] = (
                            member.name,
                            channels,
                            sample_rate,
                            audio.getnframes(),
                        )

    samples = []
    for identifier, (textgrid_member, speaker_count, turn_count) in grids.items():
        audio = wavs.get(identifier)
        if audio is None:
            raise ValueError("speaker-labeled TextGrid has no same-stem WAV")
        audio_member, channels, sample_rate, frames = audio
        if channels != 8 or sample_rate != 16000:
            raise ValueError("speaker-labeled reference is not paired with 8-channel 16 kHz audio")
        samples.append(
            Sample(
                meeting_id=meeting_id(audio_member),
                audio_member=audio_member,
                textgrid_member=textgrid_member,
                speaker_count=speaker_count,
                reference_turn_count=turn_count,
                duration_seconds=frames / sample_rate,
            )
        )
    samples.sort(key=lambda sample: sample.meeting_id)
    if len({sample.meeting_id for sample in samples}) != len(samples):
        raise ValueError("expected exactly one far-field recording per meeting")
    if len(samples) < 10 or sum(sample.duration_seconds for sample in samples) < 90 * 60:
        raise ValueError("AliMeeting Test archive does not meet the 10-sample / 90-minute minimum")
    return samples


def copy_member(tar: tarfile.TarFile, name: str, destination: Path) -> None:
    member = tar.getmember(name)
    if not member.isfile():
        raise ValueError("selected archive member is not a regular file")
    source: BinaryIO | None = tar.extractfile(member)
    if source is None:
        raise ValueError("could not read selected archive member")
    with source, destination.open("wb") as target:
        shutil.copyfileobj(source, target, length=1024 * 1024)


def prepare(archive: Path, output: Path, ffmpeg: str = "ffmpeg") -> dict:
    archive = archive.resolve(strict=True)
    output = output.resolve()
    repo_root = Path(__file__).resolve().parents[2]
    if output == repo_root or repo_root in output.parents:
        raise ValueError("prepared meeting media must remain outside the repository")
    if output.exists() and any(output.iterdir()):
        raise FileExistsError("output directory must be empty to preserve existing data")
    if shutil.which(ffmpeg) is None and not Path(ffmpeg).is_file():
        raise FileNotFoundError(f"ffmpeg executable not found: {ffmpeg}")

    samples = inspect_archive(archive)
    output.mkdir(parents=True, exist_ok=True)
    work = output / ".work"
    work.mkdir(exist_ok=True)
    by_audio = {sample.audio_member: sample for sample in samples}
    by_grid = {sample.textgrid_member: sample for sample in samples}
    sample_numbers = {sample.meeting_id: f"sample-{index:02d}" for index, sample in enumerate(samples, 1)}
    completed = set()

    with tarfile.open(archive, "r:gz") as tar:
        for member in tar:
            sample = by_audio.get(member.name)
            if sample:
                sample_id = sample_numbers[sample.meeting_id]
                raw_audio = work / f"{sample_id}.source.wav"
                wav_path = output / f"{sample_id}.wav"
                copy_member(tar, member.name, raw_audio)
                result = subprocess.run(
                    [ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i", str(raw_audio), "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", str(wav_path)],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                raw_audio.unlink(missing_ok=True)
                if result.returncode:
                    wav_path.unlink(missing_ok=True)
                    raise RuntimeError(f"ffmpeg failed for {sample_id}: {result.stderr[-1000:]}")
                with wave.open(str(wav_path), "rb") as audio:
                    if audio.getnchannels() != 1 or audio.getframerate() != 16000:
                        raise ValueError(f"converted audio format check failed for {sample_id}")
                    duration = audio.getnframes() / audio.getframerate()
                if abs(duration - sample.duration_seconds) > 0.1:
                    raise ValueError(f"converted audio duration changed for {sample_id}")
                completed.add(sample_id)
            sample = by_grid.get(member.name)
            if sample:
                sample_id = sample_numbers[sample.meeting_id]
                source = tar.extractfile(member)
                if source is None:
                    raise ValueError("could not read selected TextGrid")
                textgrid = source.read().decode("utf-8", errors="strict")
                rttm_path = output / f"{sample_id}.rttm"
                rttm_path.write_text(convert_textgrid_to_rttm(textgrid, sample_id), encoding="utf-8")

    if completed != set(sample_numbers.values()):
        raise ValueError("archive ended before every selected audio member was prepared")
    manifest_samples = []
    total_seconds = 0.0
    for sample in samples:
        sample_id = sample_numbers[sample.meeting_id]
        wav_path, rttm_path = output / f"{sample_id}.wav", output / f"{sample_id}.rttm"
        total_seconds += sample.duration_seconds
        manifest_samples.append(
            {
                "sample_id": sample_id,
                "audio_minutes": round(sample.duration_seconds / 60, 3),
                "speaker_count": sample.speaker_count,
                "reference_turn_count": sample.reference_turn_count,
                "audio_sha256": sha256(wav_path),
                "reference_sha256": sha256(rttm_path),
            }
        )
    manifest = {
        "schema_version": 1,
        "dataset": "AliMeeting Test",
        "license": "CC BY-SA 4.0; retain attribution and license conditions with private evaluation material",
        "archive_sha256": sha256(archive),
        "sample_count": len(samples),
        "total_audio_minutes": round(total_seconds / 60, 3),
        "audio_conversion": "8-channel 16 kHz PCM WAV downmixed by ffmpeg to mono 16 kHz PCM S16LE",
        "reference_source": "official Praat TextGrid speaker intervals; labels remapped to anonymous per-sample IDs",
        "samples": manifest_samples,
    }
    (output / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    (output / "ATTRIBUTION.txt").write_text(
        "AliMeeting Test corpus provided by Alibaba Group via OpenSLR (SLR119).\n"
        "License: CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/).\n"
        "Source: https://www.openslr.org/119/\n"
        "This private evaluation copy is not part of the Meetily repository.\n",
        encoding="utf-8",
    )
    shutil.rmtree(work)
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True, help="local AliMeeting Test_Ali.tar.gz")
    parser.add_argument("--output", type=Path, required=True, help="empty private output directory outside Git")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    args = parser.parse_args()
    try:
        result = prepare(args.archive, args.output, args.ffmpeg)
    except (OSError, ValueError, tarfile.TarError, wave.Error, RuntimeError) as error:
        print(f"AliMeeting preparation failed: {error}", file=sys.stderr)
        return 2
    print(json.dumps({key: value for key, value in result.items() if key != "samples"}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
