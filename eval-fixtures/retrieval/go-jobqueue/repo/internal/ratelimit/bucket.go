// Package ratelimit throttles producers so one noisy tenant cannot crowd
// out the rest.
package ratelimit

import (
	"sync"
	"time"
)

type bucket struct {
	tokens float64
	last   time.Time
}

// Limiter is a set of token buckets keyed by tenant.
type Limiter struct {
	mu        sync.Mutex
	rate      float64 // tokens per second
	burst     float64
	overrides map[string]float64
	buckets   map[string]*bucket
	now       func() time.Time
}

// New returns a limiter allowing rate requests/sec with the given burst.
func New(rate float64, burst int) *Limiter {
	return &Limiter{
		rate:      rate,
		burst:     float64(burst),
		overrides: map[string]float64{},
		buckets:   map[string]*bucket{},
		now:       time.Now,
	}
}

// SetRate gives one tenant a custom rate (e.g. a paid plan).
func (l *Limiter) SetRate(tenant string, rate float64) {
	l.mu.Lock()
	l.overrides[tenant] = rate
	l.mu.Unlock()
}

// Allow takes one token for tenant. When it returns false, wait is how long
// until a token becomes available, suitable for a Retry-After header.
func (l *Limiter) Allow(tenant string) (ok bool, wait time.Duration) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.now()
	rate := l.rate
	if r, found := l.overrides[tenant]; found {
		rate = r
	}
	b, found := l.buckets[tenant]
	if !found {
		b = &bucket{tokens: l.burst, last: now}
		l.buckets[tenant] = b
	}
	b.tokens += now.Sub(b.last).Seconds() * rate
	if b.tokens > l.burst {
		b.tokens = l.burst
	}
	b.last = now
	if b.tokens >= 1 {
		b.tokens--
		return true, 0
	}
	return false, time.Duration((1 - b.tokens) / rate * float64(time.Second))
}

// Prune drops buckets idle for longer than idle to bound memory.
func (l *Limiter) Prune(idle time.Duration) int {
	l.mu.Lock()
	defer l.mu.Unlock()
	cutoff := l.now().Add(-idle)
	n := 0
	for k, b := range l.buckets {
		if b.last.Before(cutoff) {
			delete(l.buckets, k)
			n++
		}
	}
	return n
}
