package metrics

import (
	"fmt"
	"io"
	"net/http"
	"strings"
)

// WriteText renders every series in exposition format.
func (r *Registry) WriteText(w io.Writer) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, k := range sortedKeys(r.counters) {
		fmt.Fprintf(w, "%s %g\n", k, r.counters[k])
	}
	for _, k := range sortedKeys(r.gauges) {
		fmt.Fprintf(w, "%s %g\n", k, r.gauges[k])
	}
	for _, k := range sortedKeys(r.hists) {
		h := r.hists[k]
		name, labels := splitSeries(k)
		for i, ub := range defaultBuckets {
			fmt.Fprintf(w, "%s_bucket{%sle=\"%g\"} %d\n", name, labels, ub, h.counts[i])
		}
		fmt.Fprintf(w, "%s_bucket{%sle=\"+Inf\"} %d\n", name, labels, h.total)
		fmt.Fprintf(w, "%s_sum %g\n%s_count %d\n", name, h.sum, name, h.total)
	}
}

func splitSeries(k string) (name, labels string) {
	i := strings.IndexByte(k, '{')
	if i < 0 {
		return k, ""
	}
	return k[:i], strings.TrimSuffix(k[i+1:], "}") + ","
}

// Handler serves the registry for scraping.
func (r *Registry) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		r.WriteText(w)
	})
}
