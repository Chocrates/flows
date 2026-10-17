// Package metrics keeps counters, gauges and histograms in memory and
// renders them in the Prometheus text exposition format.
package metrics

import (
	"sort"
	"strings"
	"sync"
)

var defaultBuckets = []float64{0.005, 0.01, 0.05, 0.1, 0.5, 1, 5, 10, 30, 60}

type histogram struct {
	counts []uint64
	sum    float64
	total  uint64
}

// Registry is a minimal, dependency-free metric store.
type Registry struct {
	mu       sync.Mutex
	counters map[string]float64
	gauges   map[string]float64
	hists    map[string]*histogram
}

// NewRegistry returns an empty registry.
func NewRegistry() *Registry {
	return &Registry{counters: map[string]float64{}, gauges: map[string]float64{}, hists: map[string]*histogram{}}
}

// series builds `name{k="v",...}` from alternating label pairs.
func series(name string, labels []string) string {
	if len(labels) == 0 {
		return name
	}
	var b strings.Builder
	b.WriteString(name)
	b.WriteByte('{')
	for i := 0; i+1 < len(labels); i += 2 {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteString(labels[i] + `="` + labels[i+1] + `"`)
	}
	b.WriteByte('}')
	return b.String()
}

// Inc adds one to a counter.
func (r *Registry) Inc(name string, labels ...string) { r.Add(name, 1, labels...) }

// Add adds v to a counter.
func (r *Registry) Add(name string, v float64, labels ...string) {
	if r == nil {
		return
	}
	r.mu.Lock()
	r.counters[series(name, labels)] += v
	r.mu.Unlock()
}

// Set records a gauge value.
func (r *Registry) Set(name string, v float64, labels ...string) {
	if r == nil {
		return
	}
	r.mu.Lock()
	r.gauges[series(name, labels)] = v
	r.mu.Unlock()
}

// Observe records v into a histogram.
func (r *Registry) Observe(name string, v float64, labels ...string) {
	if r == nil {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	key := series(name, labels)
	h, ok := r.hists[key]
	if !ok {
		h = &histogram{counts: make([]uint64, len(defaultBuckets))}
		r.hists[key] = h
	}
	for i, ub := range defaultBuckets {
		if v <= ub {
			h.counts[i]++
		}
	}
	h.sum += v
	h.total++
}

func sortedKeys[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
