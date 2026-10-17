package torrent

import (
	"context"
	"net"
)

type dialPool struct {
	resCh chan DialResult
	addr  string
	left  int
	hook  func(net.Conn) net.Conn
}

func (me *dialPool) getFirst() (res DialResult) {
	for me.left > 0 && res.Conn == nil {
		res = <-me.resCh
		me.left--
	}
	return
}

func (me *dialPool) add(ctx context.Context, dialer Dialer, hook func(net.Conn) net.Conn) {
	me.left++
	if hook == nil {
		hook = me.hook
	}
	go func() {
		me.resCh <- DialResult{
			dialFromSocket(ctx, dialer, me.addr, hook),
			dialer,
		}
	}()
}

func (me *dialPool) startDrainer() {
	go me.drainAndCloseRemainingDials()
}

func (me *dialPool) drainAndCloseRemainingDials() {
	for me.left > 0 {
		conn := (<-me.resCh).Conn
		me.left--
		if conn != nil {
			conn.Close()
		}
	}
}
