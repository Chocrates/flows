package schedule

import (
	"testing"
	"time"
)

func TestNextBusinessHours(t *testing.T) {
	s, err := Parse("*/15 9-17 * * 1-5")
	if err != nil {
		t.Fatal(err)
	}
	// Saturday 10:07 -> Monday 09:00
	from := time.Date(2024, 3, 9, 10, 7, 0, 0, time.UTC)
	want := time.Date(2024, 3, 11, 9, 0, 0, 0, time.UTC)
	if got := s.Next(from); !got.Equal(want) {
		t.Fatalf("got %v want %v", got, want)
	}
}

func TestParseRejectsGarbage(t *testing.T) {
	for _, expr := range []string{"", "* * *", "60 * * * *", "*/0 * * * *", "5-1 * * * *"} {
		if _, err := Parse(expr); err == nil {
			t.Errorf("%q: expected error", expr)
		}
	}
}
