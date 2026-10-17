package retry

import (
	"errors"
	"time"
)

var errBoom = errors.New("boom")

type permanentError struct{ err error }

func (p permanentError) Error() string { return "permanent: " + p.err.Error() }
func (p permanentError) Unwrap() error { return p.err }

// Permanent marks err as not worth retrying (bad input, missing resource).
// Handlers wrap errors with this to skip straight to parking the job.
func Permanent(err error) error {
	if err == nil {
		return nil
	}
	return permanentError{err}
}

// IsPermanent reports whether err was marked with Permanent.
func IsPermanent(err error) bool {
	var p permanentError
	return errors.As(err, &p)
}

// RetryAfter lets a handler request a specific delay, e.g. from an upstream
// 429 response, overriding the computed backoff.
type RetryAfter struct {
	Err   error
	After time.Duration
}

func (r *RetryAfter) Error() string { return r.Err.Error() }
func (r *RetryAfter) Unwrap() error { return r.Err }

// Policy combines an attempt budget with a delay schedule.
type Policy struct {
	MaxAttempts int
	Backoff     *Backoff
}

// ShouldRetry reports whether attempt (already made) may be followed by another.
func (p Policy) ShouldRetry(err error, attempt int) bool {
	if err == nil || IsPermanent(err) {
		return false
	}
	return attempt < p.MaxAttempts
}

// Next returns when the job should run again.
func (p Policy) Next(err error, attempt int, now time.Time) time.Time {
	var ra *RetryAfter
	if errors.As(err, &ra) && ra.After > 0 {
		return now.Add(ra.After)
	}
	return now.Add(p.Backoff.Delay(attempt))
}
