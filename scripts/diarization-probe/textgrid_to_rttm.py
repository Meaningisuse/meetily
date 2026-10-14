"""Convert Praat long-format speaker tiers to anonymous RTTM turns."""
from __future__ import annotations

import math
import re


_NUMBER = r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?"
_TIER = re.compile(
    r"(?ms)^\s*item\s+\[\d+\]:\s*\n(?P<body>.*?)(?=^\s*item\s+\[\d+\]:\s*$|\Z)"
)
_INTERVAL = re.compile(
    r"(?ms)^\s*intervals\s+\[\d+\]:\s*\n(?P<body>.*?)(?=^\s*intervals\s+\[\d+\]:\s*$|\Z)"
)
_FIELD = {
    name: re.compile(rf"(?m)^\s*{name}\s*=\s*({_NUMBER})\s*$")
    for name in ("xmin", "xmax")
}
_TEXT = re.compile(r'(?m)^\s*text\s*=\s*"(.*)"\s*$')
_SAMPLE_ID = re.compile(r"^sample-[A-Za-z0-9-]{1,57}$")


def read_speaker_turns(textgrid: str) -> list[tuple[float, float, str]]:
    """Return non-empty N_SPK interval tiers without exposing their text labels."""
    speakers: dict[str, list[tuple[float, float, str]]] = {}
    for tier_match in _TIER.finditer(textgrid):
        tier = tier_match.group("body")
        tier_class = re.search(r'(?m)^\s*class\s*=\s*"([^"]+)"\s*$', tier)
        tier_name = re.search(r'(?m)^\s*name\s*=\s*"([^"]+)"\s*$', tier)
        if not tier_class or tier_class.group(1) != "IntervalTier":
            continue
        if not tier_name or not tier_name.group(1).startswith("N_SPK"):
            continue
        speaker = tier_name.group(1)
        if speaker in speakers:
            raise ValueError("duplicate speaker tier in TextGrid")
        turns = []
        for interval_match in _INTERVAL.finditer(tier):
            interval = interval_match.group("body")
            values = {}
            for field, pattern in _FIELD.items():
                match = pattern.search(interval)
                if not match:
                    raise ValueError(f"speaker tier contains interval without {field}")
                values[field] = float(match.group(1))
            label = _TEXT.search(interval)
            if not label:
                raise ValueError("speaker tier contains interval without text field")
            # Praat escapes a quote inside a label by doubling it. Labels are used
            # only to distinguish speech from silence and are never written out.
            if not label.group(1).replace('""', '"').strip():
                continue
            start, end = values["xmin"], values["xmax"]
            if not math.isfinite(start) or not math.isfinite(end) or start < 0 or end <= start:
                raise ValueError("speaker tier contains invalid interval")
            turns.append((start, end, speaker))
        if turns:
            speakers[speaker] = turns

    if not speakers:
        raise ValueError("TextGrid contains no labeled speaker turns")
    if len(speakers) > 8:
        raise ValueError("TextGrid has more than eight speaker tiers")
    return [turn for speaker in sorted(speakers) for turn in speakers[speaker]]


def convert_textgrid_to_rttm(textgrid: str, sample_id: str) -> str:
    """Convert speaker tiers to RTTM using per-recording anonymous speaker IDs."""
    if not _SAMPLE_ID.fullmatch(sample_id):
        raise ValueError("sample ID must use the anonymous sample-<id> form")
    turns = read_speaker_turns(textgrid)
    speakers = sorted({speaker for _, _, speaker in turns})
    speaker_ids = {speaker: f"speaker_{index}" for index, speaker in enumerate(speakers, 1)}
    turns.sort(key=lambda turn: (turn[0], turn[1], speaker_ids[turn[2]]))
    return "".join(
        f"SPEAKER {sample_id} 1 {start:.6f} {end - start:.6f} <NA> <NA> "
        f"{speaker_ids[speaker]} <NA> <NA>\n"
        for start, end, speaker in turns
    )
