package job

import (
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"time"
)

// MaxPayloadBytes caps a submitted payload; larger bodies are rejected.
const MaxPayloadBytes = 256 << 10

var kindPattern = regexp.MustCompile(`^[a-z][a-z0-9_.-]{0,63}$`)

// FieldError describes one problem with a submission.
type FieldError struct {
	Field  string `json:"field"`
	Reason string `json:"reason"`
}

func (e FieldError) Error() string { return e.Field + ": " + e.Reason }

// Problems is a list of FieldErrors returned together to the client.
type Problems []FieldError

func (p Problems) Error() string {
	if len(p) == 0 {
		return "no problems"
	}
	return fmt.Sprintf("%d invalid field(s), first: %s", len(p), p[0].Error())
}

// Submission is the wire shape a producer sends.
type Submission struct {
	Kind           string          `json:"kind"`
	Payload        json.RawMessage `json:"payload"`
	Priority       *int            `json:"priority"`
	Delay          string          `json:"delay"`
	MaxAttempts    int             `json:"max_attempts"`
	IdempotencyKey string          `json:"idempotency_key"`
}

// Check inspects a submission and returns every problem found, not just the first.
func (s *Submission) Check() error {
	var p Problems
	if !kindPattern.MatchString(s.Kind) {
		p = append(p, FieldError{"kind", "must be lowercase, start with a letter, max 64 chars"})
	}
	if len(s.Payload) == 0 {
		p = append(p, FieldError{"payload", "required"})
	} else if len(s.Payload) > MaxPayloadBytes {
		p = append(p, FieldError{"payload", fmt.Sprintf("exceeds %d bytes", MaxPayloadBytes)})
	} else if !json.Valid(s.Payload) {
		p = append(p, FieldError{"payload", "not valid JSON"})
	}
	if s.Priority != nil && (*s.Priority < int(PriorityLow) || *s.Priority > int(PriorityHigh)) {
		p = append(p, FieldError{"priority", "must be between 0 and 10"})
	}
	if s.Delay != "" {
		if d, err := time.ParseDuration(s.Delay); err != nil || d < 0 {
			p = append(p, FieldError{"delay", "must be a non-negative duration like 30s"})
		}
	}
	if s.MaxAttempts < 0 || s.MaxAttempts > 50 {
		p = append(p, FieldError{"max_attempts", "must be between 0 and 50"})
	}
	if len(s.IdempotencyKey) > 128 {
		p = append(p, FieldError{"idempotency_key", "too long"})
	}
	if len(p) > 0 {
		return p
	}
	return nil
}

// ErrEmptyTenant is returned when a request carries no tenant identity.
var ErrEmptyTenant = errors.New("tenant is required")
