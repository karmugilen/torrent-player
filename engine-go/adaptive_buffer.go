package main

import "math"

// demandBufferController adds bounded headroom for variable reader demand.
// The feedback uses measured rates, not constants borrowed from natural
// patterns. It is an experimental policy, not a universality theorem.
type demandBufferController struct {
	mean, variance         float64
	window                 int64
	burstAware             bool
	fastMean, fastVariance float64
	slowMean, slowVariance float64
}

func (c *demandBufferController) observe(bytesPerSecond float64, limit int64) int64 {
	if c.mean == 0 {
		c.mean = bytesPerSecond
	} else {
		delta := bytesPerSecond - c.mean
		c.mean += 0.2 * delta
		c.variance = 0.8 * (c.variance + 0.2*delta*delta)
	}
	// Eight seconds of demand plus a variability margin. Scale comes from
	// consumption, so the same policy works for different file bitrates.
	forecast := c.mean + 2*math.Sqrt(c.variance)
	if c.burstAware {
		update := func(mean, variance *float64, weight float64) {
			if *mean == 0 {
				*mean = bytesPerSecond
				return
			}
			delta := bytesPerSecond - *mean
			*mean += weight * delta
			*variance = (1 - weight) * (*variance + weight*delta*delta)
		}
		update(&c.fastMean, &c.fastVariance, 0.5)
		update(&c.slowMean, &c.slowVariance, 0.05)
		forecast = max(forecast, c.fastMean+2*math.Sqrt(c.fastVariance), c.slowMean+2*math.Sqrt(c.slowVariance))
	}
	target := int64(min(float64(limit), 8*forecast))
	target = max(int64(4<<20), target)
	if c.window <= 0 {
		c.window = target
	}
	// Damping and a dead band keep alternating reads from constantly resizing
	// the window. The storage budget is applied separately by the scheduler.
	step := max(int64(1), c.window/4)
	if target > c.window+c.window/10 {
		c.window = min(target, c.window+step)
	} else if target < c.window-c.window/10 {
		c.window = max(target, c.window-step)
	}
	c.window = max(int64(4<<20), min(limit, c.window))
	return c.window
}
