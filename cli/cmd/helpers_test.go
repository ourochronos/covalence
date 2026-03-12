package cmd

import "testing"

func TestTruncateRunes_ASCII(t *testing.T) {
	result := truncateRunes("hello world", 5)
	if result != "hello..." {
		t.Errorf("expected \"hello...\", got %q", result)
	}
}

func TestTruncateRunes_NoTruncation(t *testing.T) {
	result := truncateRunes("short", 10)
	if result != "short" {
		t.Errorf("expected \"short\", got %q", result)
	}
}

func TestTruncateRunes_ExactLength(t *testing.T) {
	result := truncateRunes("exact", 5)
	if result != "exact" {
		t.Errorf("expected \"exact\", got %q", result)
	}
}

func TestTruncateRunes_Empty(t *testing.T) {
	result := truncateRunes("", 5)
	if result != "" {
		t.Errorf("expected empty string, got %q", result)
	}
}

func TestTruncateRunes_Emoji(t *testing.T) {
	// Each emoji is one rune but 4 bytes.
	input := "Hello\U0001F600World\U0001F601Test"
	result := truncateRunes(input, 7)
	expected := "Hello\U0001F600W..."
	if result != expected {
		t.Errorf("expected %q, got %q", expected, result)
	}
}

func TestTruncateRunes_CJK(t *testing.T) {
	// Each CJK character is one rune but 3 bytes.
	input := "\u4F60\u597D\u4E16\u754C\u6D4B\u8BD5"
	result := truncateRunes(input, 3)
	expected := "\u4F60\u597D\u4E16..."
	if result != expected {
		t.Errorf("expected %q, got %q", expected, result)
	}
}

func TestShortID_Normal(t *testing.T) {
	result := shortID("12345678-abcd-ef01-2345-6789abcdef01")
	if result != "12345678" {
		t.Errorf("expected \"12345678\", got %q", result)
	}
}

func TestShortID_Short(t *testing.T) {
	result := shortID("abc")
	if result != "abc" {
		t.Errorf("expected \"abc\", got %q", result)
	}
}

func TestGetString_Present(t *testing.T) {
	m := map[string]interface{}{"key": "value"}
	result := getString(m, "key")
	if result != "value" {
		t.Errorf("expected \"value\", got %q", result)
	}
}

func TestGetString_Missing(t *testing.T) {
	m := map[string]interface{}{}
	result := getString(m, "key")
	if result != "" {
		t.Errorf("expected empty string, got %q", result)
	}
}

func TestGetString_Nil(t *testing.T) {
	m := map[string]interface{}{"key": nil}
	result := getString(m, "key")
	if result != "" {
		t.Errorf("expected empty string, got %q", result)
	}
}

func TestGetFloat_Present(t *testing.T) {
	m := map[string]interface{}{"key": 3.14}
	result := getFloat(m, "key")
	if result != 3.14 {
		t.Errorf("expected 3.14, got %f", result)
	}
}

func TestGetFloat_Missing(t *testing.T) {
	m := map[string]interface{}{}
	result := getFloat(m, "key")
	if result != 0.0 {
		t.Errorf("expected 0.0, got %f", result)
	}
}
