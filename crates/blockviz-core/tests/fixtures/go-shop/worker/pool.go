// Package worker runs jobs on a fixed number of goroutines.
package worker

import (
	"context"
	"sync"
)

// Job is a unit of work.
type Job func(ctx context.Context) error

// Pool runs jobs concurrently.
type Pool struct {
	size int
	jobs chan Job
	wg   sync.WaitGroup
}

// NewPool makes a pool of n workers.
func NewPool(n int) *Pool {
	return &Pool{size: n, jobs: make(chan Job)}
}

// Start launches the workers.
func (p *Pool) Start(ctx context.Context, results chan<- error) {
	for i := 0; i < p.size; i++ {
		p.wg.Add(1)
		go p.work(ctx, results)
	}
}

func (p *Pool) work(ctx context.Context, results chan<- error) {
	defer p.wg.Done()
	for {
		select {
		case <-ctx.Done():
			return
		case job, ok := <-p.jobs:
			if !ok {
				return
			}
			results <- job(ctx)
		}
	}
}

// Submit queues a job.
func (p *Pool) Submit(j Job) { p.jobs <- j }

// Stop waits for the workers to finish.
func (p *Pool) Stop() {
	close(p.jobs)
	p.wg.Wait()
}
