#!/usr/bin/env python3
"""Compare frame-level subtitle detection output against SRT ground truth."""

import argparse
import bisect
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, List, Optional, Sequence, Tuple

_TIME_RANGE_SEP = "-->"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Evaluate frame-level subtitle detection JSON against an SRT subtitle file."
        )
    )
    parser.add_argument("detected_json", type=Path, help="Path to detection JSON output")
    parser.add_argument("subtitle_srt", type=Path, help="Path to SRT subtitle file")
    return parser.parse_args()


@dataclass(frozen=True)
class Detection:
    timestamp: float
    predicted: bool


def parse_detection_json(path: Path) -> Tuple[List[Detection], int]:
    with path.open("r", encoding="utf-8") as fh:
        data = json.load(fh)

    if not isinstance(data, list):
        raise ValueError("detection JSON must be an array of per-frame entries")

    detections: List[Detection] = []
    skipped = 0
    for entry in data:
        if not isinstance(entry, dict):
            skipped += 1
            continue
        timestamp = entry.get("timestamp_seconds")
        has_subtitle = entry.get("has_subtitle")
        if timestamp is None or not isinstance(timestamp, (int, float)):
            skipped += 1
            continue
        predicted = bool(has_subtitle)
        detections.append(Detection(float(timestamp), predicted))
    detections.sort(key=lambda d: d.timestamp)
    return detections, skipped


@dataclass(frozen=True)
class SubtitleInterval:
    start: float
    end: float

    def contains(self, timestamp: float) -> bool:
        return self.start <= timestamp <= self.end


def _parse_timestamp(timestamp: str) -> float:
    hours, minutes, rest = timestamp.split(":")
    seconds, millis = rest.split(",")
    return (
        int(hours) * 3600
        + int(minutes) * 60
        + int(seconds)
        + int(millis) / 1000.0
    )


def parse_srt(path: Path) -> List[SubtitleInterval]:
    intervals: List[SubtitleInterval] = []
    with path.open("r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if _TIME_RANGE_SEP in line:
                start_text, end_text = [part.strip() for part in line.split(_TIME_RANGE_SEP, 1)]
                try:
                    start = _parse_timestamp(start_text)
                    end = _parse_timestamp(end_text)
                except ValueError as exc:
                    raise ValueError(f"Invalid timestamp line in SRT: {line}") from exc
                if end < start:
                    start, end = end, start
                intervals.append(SubtitleInterval(start, end))
    intervals.sort(key=lambda interval: interval.start)
    return intervals


def build_interval_lookup(intervals: Sequence[SubtitleInterval]):
    starts = [interval.start for interval in intervals]
    return intervals, starts


def is_subtitle_timestamp(
    timestamp: float, lookup: Tuple[Sequence[SubtitleInterval], Sequence[float]]
) -> bool:
    intervals, starts = lookup
    idx = bisect.bisect_right(starts, timestamp) - 1
    if idx >= 0:
        interval = intervals[idx]
        if interval.contains(timestamp):
            return True
    return False


@dataclass
class Metrics:
    evaluated_frames: int
    skipped_frames: int
    subtitle_frames: int
    non_subtitle_frames: int
    true_positive: int
    true_negative: int
    false_positive: int
    false_negative: int

    @property
    def accuracy(self) -> Optional[float]:
        total = self.evaluated_frames
        if total == 0:
            return None
        return (self.true_positive + self.true_negative) / total

    @property
    def error_rate(self) -> Optional[float]:
        accuracy = self.accuracy
        if accuracy is None:
            return None
        return 1.0 - accuracy

    @property
    def precision(self) -> Optional[float]:
        denom = self.true_positive + self.false_positive
        if denom == 0:
            return None
        return self.true_positive / denom

    @property
    def recall(self) -> Optional[float]:
        denom = self.true_positive + self.false_negative
        if denom == 0:
            return None
        return self.true_positive / denom

    @property
    def f1(self) -> Optional[float]:
        precision = self.precision
        recall = self.recall
        if precision is None or recall is None or precision + recall == 0:
            return None
        return 2 * precision * recall / (precision + recall)


def evaluate(
    detections: Sequence[Detection], intervals: Sequence[SubtitleInterval]
) -> Metrics:
    lookup = build_interval_lookup(intervals)
    subtitle_frames = 0
    non_subtitle_frames = 0
    tp = tn = fp = fn = 0

    for detection in detections:
        truth = is_subtitle_timestamp(detection.timestamp, lookup) if intervals else False
        if truth:
            subtitle_frames += 1
        else:
            non_subtitle_frames += 1

        if detection.predicted:
            if truth:
                tp += 1
            else:
                fp += 1
        else:
            if truth:
                fn += 1
            else:
                tn += 1

    return Metrics(
        evaluated_frames=len(detections),
        skipped_frames=0,
        subtitle_frames=subtitle_frames,
        non_subtitle_frames=non_subtitle_frames,
        true_positive=tp,
        true_negative=tn,
        false_positive=fp,
        false_negative=fn,
    )


def print_report(metrics: Metrics, skipped: int) -> None:
    metrics.skipped_frames = skipped  # type: ignore[attr-defined]
    total_frames = metrics.evaluated_frames + metrics.skipped_frames

    def fmt(value: Optional[float]) -> str:
        return "n/a" if value is None else f"{value:.4f}"

    print("Subtitle Detection Evaluation")
    print("==============================")
    print(f"Frames in JSON           : {total_frames}")
    print(f"Evaluated frames         : {metrics.evaluated_frames}")
    print(f"Skipped frames (no timestamp): {skipped}")
    print(f"Ground-truth subtitle frames  : {metrics.subtitle_frames}")
    print(f"Ground-truth non-subtitle frames: {metrics.non_subtitle_frames}")
    print()
    print("Confusion Matrix")
    print("----------------")
    print(f"True Positive  : {metrics.true_positive}")
    print(f"True Negative  : {metrics.true_negative}")
    print(f"False Positive : {metrics.false_positive}")
    print(f"False Negative : {metrics.false_negative}")
    print()
    print("Metrics")
    print("-------")
    print(f"Accuracy        : {fmt(metrics.accuracy)}")
    print(f"Error rate      : {fmt(metrics.error_rate)}")
    print(f"Precision       : {fmt(metrics.precision)}")
    print(f"Recall (coverage): {fmt(metrics.recall)}")
    print(f"F1 score        : {fmt(metrics.f1)}")


def main() -> int:
    args = parse_args()
    try:
        detections, skipped = parse_detection_json(args.detected_json)
        intervals = parse_srt(args.subtitle_srt)
        metrics = evaluate(detections, intervals)
        print_report(metrics, skipped)
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
