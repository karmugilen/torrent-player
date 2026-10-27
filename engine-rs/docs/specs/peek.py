import sys, json, subprocess
names = {}
for line in open('/tmp/wt-specs/workers.txt'):
    p = line.split()
    if len(p) == 5: names[p[4]] = p[0] + '/' + p[1]
args = ['orca', 'orchestration', 'check', '--run', 'run_861102160a1b', '--json']
if len(sys.argv) > 1: args[3:3] = ['--ack', sys.argv[1]]
out = subprocess.run(args, capture_output=True, text=True, cwd='/home/kar/orca/workspaces/torrent-player/rust-engine').stdout
d = json.loads(out); r = d.get('result') or {}
if not d.get('ok'): print('ERROR', out[:1500]); sys.exit()
print('delivery', r.get('deliveryId'), 'msgs', len(r.get('messages') or []))
for m in r.get('messages') or []:
    who = names.get(m['from_handle'], m['from_handle'])
    if m['type'] == 'heartbeat':
        print('  hb', who, json.loads(m.get('payload') or '{}').get('phase'))
    else:
        print('=' * 8, m['type'], m['id'], who, m['subject']); print(m['body']); print('payload:', m.get('payload'))
