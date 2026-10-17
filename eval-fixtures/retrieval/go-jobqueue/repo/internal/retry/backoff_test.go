package retry

import (
	"testing"
	"time"
)

func TestDelayStaysWithinCeiling(t *testing.T) {
	b := NewBackoff(100*time.Millisecond, 5*time.Second)
	for attempt := 1; attempt <= 20; attempt++ {
		for i := 0; i < 50; i++ {
			d := b.Delay(attempt)
			if d > 5*time.Second {
				t.Fatalf("attempt %d: %v exceeds max", attempt, d)
			}
			if d < 50*time.Millisecond {
				t.Fatalf("attempt %d: %v below floor", attempt, d)
			}
		}
	}
}

func TestPermanentErrorsAreNotRetried(t *testing.T) {
	p := Policy{MaxAttempts: 5}
	if p.ShouldRetry(Permanent(errBoom), 1) {
		t.Fatal("permanent error should not retry")
	}
	if !p.ShouldRetry(errBoom, 1) {
		t.Fatal("transient error should retry")
	}
	if p.ShouldRetry(errBoom, 5) {
		t.Fatal("should stop at max attempts")
	}
}
