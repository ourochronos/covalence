"""Unit tests for the span-based coreference resolution with mutation tracking."""

import sys
import os
sys.path.insert(0, os.path.dirname(__file__))

from server import _resolve_with_mutations, _build_char_to_byte_map


def test_no_clusters():
    text = "Einstein published a paper."
    resolved, mutations = _resolve_with_mutations(text, [], [])
    assert resolved == text
    assert mutations == []


def test_single_pronoun_replacement():
    text = "Einstein published his theory."
    # Cluster: ["Einstein", "his"]
    # "his" at character offset 19..22 (exclusive end)
    clusters_str = [["Einstein", "his"]]
    clusters_spans = [[(0, 8), (19, 22)]]
    resolved, mutations = _resolve_with_mutations(text, clusters_str, clusters_spans)

    assert "Einstein" in resolved
    assert "his" not in resolved
    assert len(mutations) == 1

    m = mutations[0]
    assert m["canonical_token"] == "his"
    assert m["mutated_token"] == "Einstein"
    # Verify byte offsets are consistent
    assert m["canonical_end"] - m["canonical_start"] == len("his".encode("utf-8"))
    assert m["mutated_end"] - m["mutated_start"] == len("Einstein".encode("utf-8"))


def test_multiple_replacements():
    text = "Einstein won the prize. He was happy. His work was recognized."
    # Cluster: ["Einstein", "He", "His"]
    # "He" at 24..26, "His" at 38..41
    assert text[24:26] == "He"
    assert text[38:41] == "His"
    clusters_str = [["Einstein", "He", "His"]]
    clusters_spans = [[(0, 8), (24, 26), (38, 41)]]
    resolved, mutations = _resolve_with_mutations(text, clusters_str, clusters_spans)

    assert "He " not in resolved
    assert "His " not in resolved
    assert len(mutations) == 2

    # Mutations should be sorted by position
    assert mutations[0]["canonical_start"] < mutations[1]["canonical_start"]

    # First replacement: "He" -> "Einstein"
    assert mutations[0]["canonical_token"] == "He"
    assert mutations[0]["mutated_token"] == "Einstein"

    # Second replacement: "His" -> "Einstein"
    assert mutations[1]["canonical_token"] == "His"
    assert mutations[1]["mutated_token"] == "Einstein"


def test_no_replacement_when_mention_longer_than_antecedent():
    text = "He is Einstein."
    # If the antecedent is shorter than the mention (unusual),
    # we only replace mentions shorter than the antecedent.
    # Here: mention "Einstein"(6,14) is longer than antecedent "He",
    # so no replacement.
    clusters_str = [["He", "Einstein"]]
    clusters_spans = [[(0, 2), (6, 14)]]
    resolved, mutations = _resolve_with_mutations(text, clusters_str, clusters_spans)
    assert resolved == text
    assert mutations == []


def test_utf8_byte_offsets():
    # Text with multi-byte characters before the pronoun
    text = "Schr\u00f6dinger's cat. He studied it."
    # \u00f6 is 2 bytes in UTF-8
    char_to_byte = _build_char_to_byte_map(text)
    # 'S' is at byte 0, 'c' at 1, 'h' at 2, 'r' at 3, '\u00f6' at 4-5, ...
    assert char_to_byte[0] == 0  # S
    assert char_to_byte[4] == 4  # \u00f6 starts at byte 4
    assert char_to_byte[5] == 6  # next char after \u00f6 is at byte 6


def test_overlapping_spans_deduped():
    text = "He saw him there."
    assert text[0:2] == "He"
    assert text[7:10] == "him"
    # Normal non-overlapping case: "He"(0,2) and "him"(7,10) both → "John"
    clusters_str = [["John", "He", "him"]]
    clusters_spans = [[(0, 4), (0, 2), (7, 10)]]
    resolved, mutations = _resolve_with_mutations(text, clusters_str, clusters_spans)
    # (0,2) "He" -> "John" takes priority; (0,4) overlaps, skipped
    # (7,10) "him" -> "John"
    assert len(mutations) == 2
    assert mutations[0]["canonical_token"] == "He"
    assert mutations[1]["canonical_token"] == "him"


def test_empty_text():
    resolved, mutations = _resolve_with_mutations("", [], [])
    assert resolved == ""
    assert mutations == []


if __name__ == "__main__":
    test_no_clusters()
    test_single_pronoun_replacement()
    test_multiple_replacements()
    test_no_replacement_when_mention_longer_than_antecedent()
    test_utf8_byte_offsets()
    test_overlapping_spans_deduped()
    test_empty_text()
    print("All coref tests passed!")
