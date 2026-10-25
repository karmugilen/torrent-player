package main

import "container/list"

// pieceLRU orders cached pieces from least to most recently used. Every
// stream read and every 16 KiB block written touches a piece; a slice scan
// made that cost grow with the cache, so a large RAM setting played slower
// than a small one. Touch and remove are constant time here.
type pieceLRU struct {
	order list.List
	at    map[int]*list.Element
}

func (l *pieceLRU) touch(index int) {
	if l.at == nil {
		l.at = make(map[int]*list.Element)
	}
	if e, ok := l.at[index]; ok {
		l.order.MoveToBack(e)
		return
	}
	l.at[index] = l.order.PushBack(index)
}

func (l *pieceLRU) remove(index int) {
	if e, ok := l.at[index]; ok {
		l.order.Remove(e)
		delete(l.at, index)
	}
}

func (l *pieceLRU) len() int { return len(l.at) }

// oldest returns the least recently used piece other than skip.
func (l *pieceLRU) oldest(skip int) (int, bool) {
	for e := l.order.Front(); e != nil; e = e.Next() {
		if index := e.Value.(int); index != skip {
			return index, true
		}
	}
	return 0, false
}

// each visits pieces from least to most recently used until visit is false.
func (l *pieceLRU) each(visit func(index int) bool) {
	for e := l.order.Front(); e != nil; e = e.Next() {
		if !visit(e.Value.(int)) {
			return
		}
	}
}

func (l *pieceLRU) reset() {
	l.order.Init()
	l.at = nil
}
