// Package retry decides when a failed job runs again.
package retry

import (
	"math"
	"math/rand"
	"sync"
	"time"
)

// Backoff computes delays that grow geometrically with the attempt number,
// with "full jitter" so that many jobs failing together do not all come
// back at the same instant.
type Backoff struct {
	Base   time.Duration
	Max    time.Duration
	Factor float64

	mu  sync.Mutex
	rnd *rand.Rand
}

// NewBackoff returns a Backoff seeded from the clock.
func NewBackoff(base, max time.Duration) *Backoff {
	return &Backoff{Base: base, Max: max, Factor: 2, rnd: rand.New(rand.NewSource(time.Now().UnixNano()))}
}

// Delay returns the wait before the given attempt (1-based).
func (b *Backoff) Delay(attempt int) time.Duration {
	if attempt < 1 {
		attempt = 1
	}
	ceiling := float64(b.Base) * math.Pow(b.Factor, float64(attempt-1))
	if ceiling > float64(b.Max) || math.IsInf(ceiling, 0) {
		ceiling = float64(b.Max)
	}
	b.mu.Lock()
	r := b.rnd.Float64()
	b.mu.Unlock()
	d := time.Duration(r * ceiling)
	if d < b.Base/2 {
		d = b.Base / 2
	}
	return d
}
