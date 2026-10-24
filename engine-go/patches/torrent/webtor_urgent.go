package torrent

import "time"

// Webtor seek extension. Urgent (PiecePriorityNow) blocks normally stay with
// whichever peer first requested them unless the upstream "don't steal from
// the poor" rule allows a move. After a seek that lets one slow peer hold the
// blocks the player is waiting for. With urgent stealing enabled, a block of an
// urgent piece that has been outstanding longer than the configured age moves
// to a requesting peer that delivered useful data more recently.

const peerUpdateRequestsUrgentNudge updateRequestReason = "webtor urgent nudge"

// SetUrgentRequestStealing enables stealing of stalled urgent blocks after the
// given age. Zero or negative disables it, restoring upstream behaviour.
func (cl *Client) SetUrgentRequestStealing(after time.Duration) {
	if cl == nil {
		return
	}
	cl.urgentStealAfter.Store(int64(max(after, 0)))
}

func (cl *Client) urgentStealAge() time.Duration {
	if cl == nil {
		return 0
	}
	return time.Duration(cl.urgentStealAfter.Load())
}

// shouldStealUrgent reports whether p may take r from existing even though the
// upstream balance rule declined. Called with the client lock held.
func (p *PeerConn) shouldStealUrgent(r RequestIndex, existing *PeerConn) bool {
	t := p.t
	after := t.cl.urgentStealAge()
	if after <= 0 {
		return false
	}
	if t.piece(t.pieceIndexOfRequestIndex(r)).purePriority() < PiecePriorityNow {
		return false
	}
	state, ok := t.requestState[r]
	if !ok || time.Since(state.when) < after {
		return false
	}
	// Only a peer that is currently delivering may take the block, and never
	// from a peer that is delivering more recently than the stealer.
	if p.lastUsefulChunkReceived.IsZero() || time.Since(p.lastUsefulChunkReceived) > after {
		return false
	}
	return p.lastUsefulChunkReceived.After(existing.lastUsefulChunkReceived)
}

// NudgeUrgentRequests asks connected peers that hold an urgent missing piece
// and have spare request slots to recompute their requests now, rather than
// waiting for their pipeline to drain. With urgent stealing enabled this is
// when stalled urgent blocks move. Returns the number of peers nudged.
func (t *Torrent) NudgeUrgentRequests() int {
	t.cl.lock()
	defer t.cl.unlock()
	if t.closed.IsSet() || !t.haveInfo() || t.dataDownloadDisallowed.Bool() {
		return 0
	}
	var urgent []pieceIndex
	t._pendingPieces.Iterate(func(x uint32) bool {
		i := pieceIndex(x)
		if !t.pieceComplete(i) && t.piece(i).purePriority() >= PiecePriorityNow {
			urgent = append(urgent, i)
		}
		return true
	})
	if len(urgent) == 0 {
		return 0
	}
	nudged := 0
	for c := range t.conns {
		if c.closed.IsSet() || c.needRequestUpdate != "" {
			continue
		}
		pending := maxRequests(c.requestState.Requests.GetCardinality() + c.requestState.Cancelled.GetCardinality())
		if pending >= c.nominalMaxRequests() {
			continue
		}
		for _, i := range urgent {
			if !c.peerHasPiece(i) {
				continue
			}
			if c.peerChoking && !c.peerAllowedFast.Contains(i) {
				continue
			}
			c.onNeedUpdateRequests(peerUpdateRequestsUrgentNudge)
			nudged++
			break
		}
	}
	return nudged
}
