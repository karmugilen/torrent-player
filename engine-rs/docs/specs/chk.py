import sys, json, subprocess
# usage: chk.py [ack_delivery_id] [timeout_ms]
names = {}
for line in open('/tmp/wt-specs/workers.txt'):
    p = line.split()
    if len(p) == 5: names[p[4]] = p[0] + '/' + p[1]
args = ['orca', 'orchestration', 'check', '--wait', '--types', 'worker_done,escalation,question', '--timeout-ms', sys.argv[2] if len(sys.argv) > 2 else '540000', '--json']
if len(sys.argv) > 1 and sys.argv[1] != '-': args[3:3] = ['--ack', sys.argv[1]]
out = subprocess.run(args, capture_output=True, text=True, cwd='/home/kar/orca/workspaces/torrent-player/rust-engine').stdout
docs, dec, i = [], json.JSONDecoder(), 0
while i < len(out):
    while i < len(out) and out[i].isspace(): i += 1
    if i >= len(out): break
    o, i = dec.raw_decode(out, i); docs.append(o)
final = [d for d in docs if not d.get('_keepalive')]
if not final: print('NO FINAL DOC', out[-500:]); sys.exit()
d = final[-1]
if not d.get('ok'): print('ERROR', json.dumps(d)[:1500]); sys.exit()
r = d['result']; print('delivery', r.get('deliveryId'), 'msgs', len(r.get('messages') or []))
for m in r.get('messages') or []:
    who = names.get(m['from_handle'], m['from_handle'])
    if m['type'] == 'heartbeat':
        print('  hb', who, json.loads(m.get('payload') or '{}').get('phase'))
    else:
        print('=' * 8, m['type'], m['id'], who, m['subject']); print(m['body']); print('payload:', m.get('payload'))
