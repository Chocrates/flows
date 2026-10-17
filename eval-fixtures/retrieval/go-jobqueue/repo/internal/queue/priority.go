package queue

import (
	"container/heap"
	"sync"

	"github.com/acme/jobqueue/internal/job"
)

// readyHeap orders claimed jobs so the highest priority, oldest job pops first.
type readyHeap []*job.Job

func (h readyHeap) Len() int { return len(h) }
func (h readyHeap) Less(a, b int) bool {
	if h[a].Priority != h[b].Priority {
		return h[a].Priority > h[b].Priority
	}
	return h[a].RunAt.Before(h[b].RunAt)
}
func (h readyHeap) Swap(a, b int)  { h[a], h[b] = h[b], h[a] }
func (h *readyHeap) Push(x any)    { *h = append(*h, x.(*job.Job)) }
func (h *readyHeap) Pop() any {
	old := *h
	n := len(old)
	j := old[n-1]
	old[n-1] = nil
	*h = old[:n-1]
	return j
}

// Buffer holds jobs that were claimed from storage but not yet handed to a
// worker. It is bounded so a slow pool does not hoard leases.
type Buffer struct {
	mu    sync.Mutex
	h     readyHeap
	cap   int
	ready chan struct{}
}

// NewBuffer returns a buffer that holds at most capacity jobs.
func NewBuffer(capacity int) *Buffer {
	return &Buffer{cap: capacity, ready: make(chan struct{}, 1)}
}

// Free reports how many more jobs the buffer will accept.
func (b *Buffer) Free() int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.cap - len(b.h)
}

// Add inserts jobs, signalling any waiter.
func (b *Buffer) Add(jobs ...*job.Job) {
	b.mu.Lock()
	for _, j := range jobs {
		heap.Push(&b.h, j)
	}
	b.mu.Unlock()
	select {
	case b.ready <- struct{}{}:
	default:
	}
}

// Next pops the best job or returns nil when empty.
func (b *Buffer) Next() *job.Job {
	b.mu.Lock()
	defer b.mu.Unlock()
	if len(b.h) == 0 {
		return nil
	}
	return heap.Pop(&b.h).(*job.Job)
}

// Wait returns a channel that fires after Add.
func (b *Buffer) Wait() <-chan struct{} { return b.ready }

// Drain removes and returns every buffered job, used on shutdown so their
// leases can be released instead of waiting to expire.
func (b *Buffer) Drain() []*job.Job {
	b.mu.Lock()
	defer b.mu.Unlock()
	out := make([]*job.Job, len(b.h))
	copy(out, b.h)
	b.h = b.h[:0]
	return out
}
