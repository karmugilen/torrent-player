//go:build !android && !jnitest

package main

import (
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"
)

func runEngine(ctlPort int, downloadDir string, maxPeers int) int {
	server, err := NewEngineServer(ctlPort, downloadDir, maxPeers)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	if err := server.Start(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		server.Close()
		return 1
	}
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGINT, syscall.SIGTERM)
	select {
	case <-signals:
		server.Close()
	case <-server.Done():
	}
	return 0
}

func main() {
	port := flag.Int("port", 18080, "control server port")
	path := flag.String("path", "/tmp/webtorrent", "download directory")
	peers := flag.Int("max-peers", 55, "maximum peers")
	flag.Parse()
	os.Exit(runEngine(*port, *path, *peers))
}
