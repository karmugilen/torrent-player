#!/usr/bin/env python3
"""Compare public swarms, one host at a time, with 5-second diagnostics.

Usage: peers_compare.py <engine-host> <label: go-*|rust-*> <magnet> <seconds>
Ubuntu magnet: magnet:?xt=urn:btih:d160b8d8ea35a5b4e52837468fc8f03d55cef1f7&tr=https%3A%2F%2Ftorrent.ubuntu.com%2Fannounce
A result.json and host.log are retained in ~/.cache/wt-tmp/peers-<label>.
"""
import collections
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request


def main():
    binary, label, magnet, seconds = sys.argv[1:]
    seconds = int(seconds)
    work = pathlib.Path.home() / '.cache/wt-tmp' / f'peers-{label}'
    work.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(work / 'data', ignore_errors=True)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    dash = '-' if label.startswith('go-') else '--'
    result = {'label': label, 'requested_seconds': seconds, 'samples': []}
    stop = threading.Event()
    process = None

    def call(method, path, body=None, timeout=10):
        req = urllib.request.Request(
            f'http://127.0.0.1:{port}{path}', method=method,
            data=json.dumps(body).encode() if body is not None else None)
        with urllib.request.urlopen(req, timeout=timeout) as response:
            return json.loads(response.read() or b'null')

    try:
        with open(work / 'host.log', 'wb') as log:
            process = subprocess.Popen(
                [binary, dash+'port', str(port), dash+'path', str(work / 'data'),
                 dash+'max-peers', '55'], stdout=log, stderr=subprocess.STDOUT,
                env={**os.environ, 'WT_SWARM_DIAGNOSTICS': '1'})
        for _ in range(100):
            if process.poll() is not None:
                raise RuntimeError(f'host exited during startup: {process.returncode}')
            try:
                call('GET', '/stats', timeout=1)
                break
            except (OSError, urllib.error.URLError):
                time.sleep(0.1)
        else:
            raise RuntimeError('host did not become ready in 10 seconds')
        call('POST', '/settings', {
            'maxPeers': 55, 'dialRatePerSec': 30, 'peerExploration': True,
            'claudeSeekMode': True, 'hedgeRequests': True, 'slowPeerYield': True,
            'dropChokers': True, 'seedingEnabled': True,
        })
        started = time.monotonic()
        tid = call('POST', '/add', {'torrentId': magnet, 'prepare': True})['id']
        while True:
            status = call('GET', f'/torrent/{tid}')
            if status.get('ready') or time.monotonic() - started > 90:
                break
            time.sleep(0.5)
        result['metadata_s'] = round(time.monotonic() - started, 2)
        files = status.get('files') or []
        if not status.get('ready') or not files:
            result['torrent'] = status
            raise RuntimeError('metadata unavailable after 90 seconds')
        index = max(files, key=lambda f: f['length'])['index']
        call('POST', '/configure', {
            'id': tid, 'descriptors': [None] * len(files), 'selected': [index],
            'storageMode': 'stream', 'streamCacheBytes': 64 << 20,
        }, timeout=70)
        call('POST', '/focus', {'id': tid, 'fileIndex': index})
        play = call('POST', '/play', {'id': tid, 'fileIndex': index, 'offset': 0})
        reader_state = {'bytes': 0}

        def reader():
            try:
                with urllib.request.urlopen(play['streamUrl'], timeout=120) as response:
                    while not stop.is_set():
                        chunk = response.read(1 << 16)
                        if not chunk:
                            break
                        reader_state['bytes'] += len(chunk)
            except Exception as error:
                reader_state['error'] = str(error)

        threading.Thread(target=reader, daemon=True).start()
        started = time.monotonic()
        with open(work / 'diagnostics.jsonl', 'w') as diagnostics:
            for sample_at in range(5, seconds + 1, 5):
                stop.wait(max(0, started + sample_at - time.monotonic()))
                status = call('GET', f'/torrent/{tid}')
                conns = call('GET', f'/connections/{tid}')
                peers = conns.get('peers') or []
                top = sorted(peers, key=lambda p: p.get('downloadRate', 0), reverse=True)[:5]
                sample = {
                    'elapsed_s': round(time.monotonic() - started, 2),
                    'peers': status.get('numPeers', 0),
                    'mib_s': round(status.get('downloadSpeed', 0) / 2**20, 3),
                    'downloaded': status.get('downloaded', 0),
                    'top5': top, 'swarm': conns.get('diagnostics'),
                    'reader': dict(reader_state),
                }
                result['samples'].append(sample)
                diagnostics.write(json.dumps(sample) + '\n')
                diagnostics.flush()
                print(json.dumps({'label': label, 'sample': sample}), flush=True)
        samples = result['samples']
        result['peers_speed_every_5s'] = [[s['peers'], s['mib_s']] for s in samples]
        result['avg_mib_s'] = round(sum(s['mib_s'] for s in samples) / len(samples), 3)
        result['peak_mib_s'] = max(s['mib_s'] for s in samples)
        result['peers_min_max'] = [min(s['peers'] for s in samples), max(s['peers'] for s in samples)]
        result['conn_summary'] = {
            'n': len(peers), 'transports': dict(collections.Counter(p.get('transport') for p in peers)),
            'downloaded_gt0': sum(p.get('downloaded', 0) > 0 for p in peers),
            'total_downloaded': sum(p.get('downloaded', 0) for p in peers),
        }
        result['reader'] = dict(reader_state)
        result['trackers'] = status.get('trackers')
    except Exception as error:
        result['error'] = f'{type(error).__name__}: {error}'
    finally:
        stop.set()
        if process is not None:
            process.terminate()
            try:
                process.wait(5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            result['host_exit_code'] = process.returncode
        shutil.rmtree(work / 'data', ignore_errors=True)
        (work / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result), flush=True)
    return 2 if 'error' in result else 0


if __name__ == '__main__':
    sys.exit(main())
