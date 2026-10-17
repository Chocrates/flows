package ratelimit

import (
	"testing"
	"time"
)

func TestTenantsAreIsolated(t *testing.T) {
	now := time.Unix(0, 0)
	l := New(1, 2)
	l.now = func() time.Time { return now }

	for i := 0; i < 2; i++ {
		if ok, _ := l.Allow("noisy"); !ok {
			t.Fatalf("burst token %d denied", i)
		}
	}
	ok, wait := l.Allow("noisy")
	if ok || wait <= 0 {
		t.Fatalf("expected denial with wait, got ok=%v wait=%v", ok, wait)
	}
	if ok, _ := l.Allow("quiet"); !ok {
		t.Fatal("other tenant should be unaffected")
	}

	now = now.Add(time.Second)
	if ok, _ := l.Allow("noisy"); !ok {
		t.Fatal("token should refill after one second")
	}
}
