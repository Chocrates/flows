// Package schedule submits jobs on a recurring timetable.
package schedule

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

// Spec is a parsed five-field expression: minute hour day-of-month month day-of-week.
type Spec struct {
	minute, hour, dom, month, dow uint64 // bitsets
}

type bounds struct{ lo, hi int }

var fieldBounds = []bounds{{0, 59}, {0, 23}, {1, 31}, {1, 12}, {0, 6}}

var aliases = map[string]string{
	"@hourly":  "0 * * * *",
	"@daily":   "0 0 * * *",
	"@weekly":  "0 0 * * 0",
	"@monthly": "0 0 1 * *",
}

// Parse reads an expression such as "*/15 9-17 * * 1-5".
func Parse(expr string) (Spec, error) {
	if a, ok := aliases[expr]; ok {
		expr = a
	}
	fields := strings.Fields(expr)
	if len(fields) != 5 {
		return Spec{}, fmt.Errorf("expected 5 fields, got %d", len(fields))
	}
	var sets [5]uint64
	for i, f := range fields {
		s, err := parseField(f, fieldBounds[i])
		if err != nil {
			return Spec{}, fmt.Errorf("field %d %q: %w", i+1, f, err)
		}
		sets[i] = s
	}
	return Spec{sets[0], sets[1], sets[2], sets[3], sets[4]}, nil
}

func parseField(f string, b bounds) (uint64, error) {
	var set uint64
	for _, part := range strings.Split(f, ",") {
		step := 1
		if i := strings.IndexByte(part, '/'); i >= 0 {
			n, err := strconv.Atoi(part[i+1:])
			if err != nil || n <= 0 {
				return 0, fmt.Errorf("bad step")
			}
			step, part = n, part[:i]
		}
		lo, hi := b.lo, b.hi
		if part != "*" {
			if i := strings.IndexByte(part, '-'); i >= 0 {
				var err1, err2 error
				lo, err1 = strconv.Atoi(part[:i])
				hi, err2 = strconv.Atoi(part[i+1:])
				if err1 != nil || err2 != nil {
					return 0, fmt.Errorf("bad range")
				}
			} else {
				v, err := strconv.Atoi(part)
				if err != nil {
					return 0, err
				}
				lo, hi = v, v
			}
		}
		if lo < b.lo || hi > b.hi || lo > hi {
			return 0, fmt.Errorf("out of range %d-%d", b.lo, b.hi)
		}
		for v := lo; v <= hi; v += step {
			set |= 1 << uint(v)
		}
	}
	return set, nil
}

// Next returns the first matching minute strictly after t, in t's location.
func (s Spec) Next(t time.Time) time.Time {
	t = t.Truncate(time.Minute).Add(time.Minute)
	limit := t.AddDate(5, 0, 0)
	for t.Before(limit) {
		switch {
		case s.month&(1<<uint(t.Month())) == 0:
			t = time.Date(t.Year(), t.Month()+1, 1, 0, 0, 0, 0, t.Location())
		case s.dom&(1<<uint(t.Day())) == 0 || s.dow&(1<<uint(t.Weekday())) == 0:
			t = time.Date(t.Year(), t.Month(), t.Day()+1, 0, 0, 0, 0, t.Location())
		case s.hour&(1<<uint(t.Hour())) == 0:
			t = t.Truncate(time.Hour).Add(time.Hour)
		case s.minute&(1<<uint(t.Minute())) == 0:
			t = t.Add(time.Minute)
		default:
			return t
		}
	}
	return time.Time{}
}
