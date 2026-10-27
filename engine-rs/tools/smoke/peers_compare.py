#!/usr/bin/env python3
"""Go vs Rust peer count and speed on a real swarm, following the app flow.

Usage: peers_compare.py <engine-host> <label: go-*|rust-*> <magnet> <seconds>
Ubuntu magnet: magnet:?xt=urn:btih:d160b8d8ea35a5b4e52837468fc8f03d55cef1f7&tr=https%3A%2F%2Ftorrent.ubuntu.com%2Fannounce
"""
import json, subprocess, sys, threading, time, urllib.request, socket, shutil, os
BIN, LABEL, MAGNET, SECS = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
work = f'/home/kar/.cache/wt-tmp/peers-{LABEL}'; shutil.rmtree(work, ignore_errors=True); os.makedirs(work)
s = socket.socket(); s.bind(('127.0.0.1', 0)); PORT = s.getsockname()[1]; s.close()
dash = '-' if 'go' in LABEL else '--'
p = subprocess.Popen([BIN, dash+'port', str(PORT), dash+'path', work + '/data', dash+'max-peers', '55'], stdout=open(work+'/host.log','wb'), stderr=subprocess.STDOUT)
def call(m, path, body=None, timeout=70):
    req = urllib.request.Request(f'http://127.0.0.1:{PORT}{path}', method=m, data=json.dumps(body).encode() if body is not None else None)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r: return r.status, json.loads(r.read() or b'null')
    except urllib.error.HTTPError as e: return e.code, e.read().decode()
stop = False
try:
    for _ in range(100):
        try: call('GET', '/stats', timeout=1); break
        except Exception: time.sleep(0.1)
    # Match Android LibrarySession defaults used by smoke.py --profile app.
    call('POST', '/settings', {'maxPeers': 55, 'dialRatePerSec': 30, 'peerExploration': True})
    t0 = time.monotonic(); code, add = call('POST', '/add', {'torrentId': MAGNET, 'prepare': True}); tid = add['id']
    while True:
        code, st = call('GET', f'/torrent/{tid}')
        if st.get('ready') or time.monotonic() - t0 > 90: break
        time.sleep(0.5)
    meta = time.monotonic() - t0
    files = st['files']; v = max(files, key=lambda f: f['length']); idx = v['index']
    call('POST', '/configure', {'id': tid, 'descriptors': [None]*len(files), 'selected': [idx], 'storageMode': 'stream', 'streamCacheBytes': 64 << 20})
    call('POST', '/focus', {'id': tid, 'fileIndex': idx})
    code, play = call('POST', '/play', {'id': tid, 'fileIndex': idx, 'offset': 0}); url = play['streamUrl']
    def reader():
        try:
            with urllib.request.urlopen(url, timeout=120) as r:
                while not stop and r.read(1 << 16): pass
        except Exception as e: pass
    threading.Thread(target=reader, daemon=True).start()
    samples = []
    for i in range(SECS // 5):
        time.sleep(5); code, st = call('GET', f'/torrent/{tid}')
        samples.append((st.get('numPeers'), round(st.get('downloadSpeed', 0) / 2**20, 2)))
    trk = [(t['url'].split('/')[2][:28], t.get('status'), t.get('peers', t.get('numPeers'))) for t in st.get('trackers', [])]
    code, conns = call('GET', f'/connections/{tid}') if True else (0, None)
    print(json.dumps({'label': LABEL, 'metadata_s': round(meta, 1), 'peers_speed_every_5s': samples, 'trackers': trk}))
    if isinstance(conns, dict): print('conns', json.dumps(conns)[:600])
finally:
    stop = True; p.terminate(); p.wait(5); shutil.rmtree(work + '/data', ignore_errors=True)
