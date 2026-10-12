import json, html
from pathlib import Path
out=Path('/home/adi/dev/rs/rcdu/docs/diagrams')
elements=[]; svg=[]; counter=0
colors={'blue':('#1971c2','#e7f5ff'),'green':('#2b8a3e','#ebfbee'),'orange':('#e67700','#fff4e6'),'purple':('#6741d9','#f3f0ff'),'gray':('#495057','#f8f9fa')}
def base(kind,x,y,w,h,stroke='#343a40',background='transparent'):
 global counter
 counter+=1
 return dict(id=f'rcdu-{counter}',type=kind,x=x,y=y,width=w,height=h,angle=0,strokeColor=stroke,backgroundColor=background,fillStyle='solid',strokeWidth=1.5,strokeStyle='solid',roughness=0,opacity=100,groupIds=[],frameId=None,index=None,roundness=None,seed=counter*7919,version=1,versionNonce=counter*3571,isDeleted=False,boundElements=None,updated=0,link=None,locked=False)
def text(x,y,value,size=16,color='#343a40',container=None):
 lines=value.split('\n'); w=max(map(len,lines))*size*.61; h=len(lines)*size*1.25
 e=base('text',x,y,w,h,color); e.update(text=value,originalText=value,fontSize=size,fontFamily=3,textAlign='left',verticalAlign='top',containerId=container,autoResize=True,lineHeight=1.25);elements.append(e)
 for i,line in enumerate(lines): svg.append(f'<text x="{x}" y="{y+size+i*size*1.25}" fill="{color}" font-family="monospace" font-size="{size}">{html.escape(line)}</text>')
 return e
nodes={}
def box(key,x,y,w,h,label,color='blue'):
 stroke,bg=colors[color]; e=base('rectangle',x,y,w,h,stroke,bg); e['roundness']={'type':3};elements.append(e)
 t=text(x+16,y+16,label,14,stroke,e['id']); e['boundElements']=[{'id':t['id'],'type':'text'}];nodes[key]=(x,y,w,h)
 svg.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" stroke="{stroke}" fill="{bg}" stroke-width="1.5"/>')
 # place background before the text
 n=len(label.split('\n')); svg[-n-1:]=[svg[-1]]+svg[-n-1:-1]
def arrow(points,label=None,at=None,dashed=False,color='#868e96'):
 x,y=points[0]; pts=[[a-x,b-y] for a,b in points];e=base('arrow',x,y,max(a for a,b in pts)-min(a for a,b in pts),max(b for a,b in pts)-min(b for a,b in pts),color)
 e.update(points=pts,startBinding=None,endBinding=None,startArrowhead=None,endArrowhead='arrow',elbowed=False);e['strokeStyle']='dashed' if dashed else 'solid';elements.append(e)
 p=' '.join(f'{a},{b}' for a,b in points);dash=' stroke-dasharray="6 5"' if dashed else ''
 svg.append(f'<polyline points="{p}" fill="none" stroke="{color}" stroke-width="1.8" marker-end="url(#arrow)"{dash}/>')
 if label: text(*at,label,14,color)
def panel(y,h,title,subtitle):
 e=base('rectangle',30,y,1940,h,'#dee2e6','#ffffff');elements.append(e);svg.append(f'<rect x="30" y="{y}" width="1940" height="{h}" rx="14" fill="white" stroke="#dee2e6"/>');text(55,y+20,title,24);text(55,y+55,subtitle,14,'#868e96')
text(40,20,'rcdu | before compatibility removal',32)
text(40,66,'Working-tree source before cleanup | Linux / Rust | 2026-10-08',16,'#868e96')
panel(110,680,'01  Session and data paths','Solid arrows: execution / data flow. Dashed arrows: control / feedback. Module names point to src/.')
box('cli',70,220,350,130,'main.rs -> session::run\ncli.rs: defaults + config\n/etc/ncdu.conf, user config\nthen command-line overrides')
box('session',480,220,390,130,'session.rs: choose source / UI\nInstall signal guards\nUI: none / line / ncurses\nHelp and version exit early')
arrow([(420,285),(480,285)])
box('live',950,220,420,130,'Live filesystem\nscan::scan_with_progress\nProgress callback + cancellation\nReturns fully accounted Model','green')
box('json',950,415,420,130,'JSON / zstd JSON import\n-f FILE or stdin (-)\nsession::read_json\nformat/json.rs -> Builder','purple')
box('ex1',950,610,420,130,'EX1 binary import (-f FILE)\nformat/binary.rs: Reader\nSeekable file + indexed blocks\nzstd block cache','purple')
arrow([(870,285),(950,285)])
arrow([(870,305),(905,305),(905,480),(950,480)])
arrow([(870,325),(885,325),(885,675),(950,675)])
box('model',1460,220,450,130,'Model (model.rs + storage.rs)\nWorker Parts + directory registry\nEntries / names / spans / totals\nHardlink + extended side records','green')
arrow([(1370,285),(1460,285)])
arrow([(1370,480),(1415,480),(1415,325),(1460,325)])
arrow([(1370,635),(1435,635),(1435,340),(1460,340)],'full import for export / quit',(1455,365))
box('export',1460,415,450,120,'Export / report / exit\nJSON, zstd JSON or EX1 writer\n--scan-report: metrics JSON\n--quit-after-scan: no browser','purple')
arrow([(1760,350),(1760,415)])
box('browser',1460,610,450,130,'browser.rs + display.rs\nSort, navigate, help, details\nCurses: render + key loop\nModel-backed OR indexed view','orange')
arrow([(1890,350),(1940,350),(1940,675),(1910,675)])
arrow([(1370,700),(1460,700)],'lazy directory listings',(1375,750))
text(70,420,'Terminal ownership',18,'#e67700')
text(70,460,'os/linux/curses.rs wraps ncursesw.\nThe session/browser thread owns the UI.\nSignals, q and terminal errors cancel scans.\nGuards restore terminal state on exit.',16)
text(70,585,'EX1 interactive import bypasses full-tree loading.\nIts browser loads the current directory and retains\nancestry frames; deletion and refresh are disabled.\nImported models disable mutations by default.',16)
panel(820,590,'02  Parallel scanning and compact storage','scan.rs exposes directory jobs AND metadata jobs, allowing several workers to scan one wide directory.')
box('root',70,940,320,145,'Open + verify root descriptor\n-t 0: available CPU count\n-t N: explicit worker count\nDescriptor budget limits pool\nos/linux.rs','blue')
box('queue',440,940,320,145,'Shared bounded job queue\nMutex + Condvar\nDirectory ID jobs\nMetadata batch jobs\nOutstanding job counter','blue')
box('enum',810,940,320,145,'Any scan worker enumerates\ngetdents64: 32 KiB buffer\nUp to 256 names per batch\nShared parent descriptor\nRaw basename bytes','green')
box('observe',1180,940,340,145,'Any worker observes metadata\nfstatat + classify + exclusions\nFilesystem / cache / kernel rules\nDiscover child directory IDs\nAccumulate ordinary subtotal','green')
box('parts',1570,940,340,145,'Append to worker-owned Part\n24-byte Entry records\n4096-entry storage chunks\n64 KiB raw-name chunks\nDirectory spans + side records','green')
for a,b in [(390,440),(760,810),(1130,1180),(1520,1570)]: arrow([(a,1010),(b,1010)])
arrow([(965,940),(965,905),(600,905),(600,940)],'publish metadata batches',(660,912),True)
arrow([(1260,1085),(1260,1130),(590,1130),(590,1085)],'publish discovered directories',(740,1138),True)
text(70,1190,'Queue full? Metadata runs locally.\nDirectory overflow keeps IDs only;\nredistribute when capacity returns.\nNo recursive worker call stack.',16)
box('finish',810,1220,710,135,'Join worker Parts without copying every file\nmodel::finish_accounting()\n1. Reverse directory reduction: ordinary totals + errors\n2. Group hardlinks by (device, inode), count once per ancestor\nIncludes directory own size; shared usage; saturating sums','blue')
arrow([(1740,1085),(1740,1290),(1520,1290)])
text(1570,1340,'Result: Model -> session / browser',15,'#2b8a3e')
panel(1440,430,'03  Browser actions and model updates','browser.rs gates actions by config/source; display.rs handles byte escaping, Unicode widths, sizes and natural sorting.')
box('actions',70,1550,350,145,'Browser key loop\nr: refresh current directory\nd: confirm / delete selection\nb: launch shell\nNavigation + sort + toggles','orange')
box('rescan',500,1540,410,110,'Refresh: verified directory descriptor\nscan::scan_descriptor -> subtree Model\nCancellation + progress on UI thread','green')
box('delete',500,1700,410,110,'delete.rs: observed-tree deletion\nVerified descriptor-relative unlinkat\nOutcome: removed IDs / links / abort','orange')
box('rebuild',1020,1590,410,130,'mutation::rebuild\nReplace subtree OR omit removed IDs\nAdjust surviving hardlink counts\nBuilder::finish -> recount totals','green')
box('install',1520,1590,390,130,'Browser::install_model\nRestore location by path\nRestore selection by name\nReload + draw updated listing','orange')
arrow([(420,1590),(500,1590)])
arrow([(420,1660),(460,1660),(460,1755),(500,1755)])
arrow([(910,1595),(970,1595),(970,1620),(1020,1620)])
arrow([(910,1755),(970,1755),(970,1680),(1020,1680)])
arrow([(1430,1655),(1520,1655)])
text(1020,1770,'Shell / custom delete command: suspend curses,\nrun child with terminal stdio, then resume.\nBuilt-in delete also refreshes when enabled.',16)
text(40,1900,'Scope: snapshot before cleanup, including uncommitted source changes. Historical reference/ncdu and archived engine excluded.',14,'#868e96')
scene={'type':'excalidraw','version':2,'source':'https://excalidraw.com','elements':elements,'appState':{'viewBackgroundColor':'#f8f9fa','gridSize':None,'scrollX':0,'scrollY':0,'zoom':{'value':0.65}},'files':{}}
(out/'rcdu-before-cleanup.excalidraw.json').write_text(json.dumps(scene,indent=2)+'\n')
header='<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="1950" viewBox="0 0 2000 1950"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="#868e96"/></marker></defs><rect width="2000" height="1950" fill="#f8f9fa"/>'
(out/'rcdu-before-cleanup.svg').write_text(header+'\n'.join(svg)+'</svg>\n')
assert len({e['id'] for e in elements})==len(elements)
ids={e['id']:e for e in elements}
for e in elements:
 if e.get('containerId'): assert e['containerId'] in ids
 for bound in e.get('boundElements') or []: assert bound['id'] in ids
print(f'Created editable scene: {len(elements)} elements; references validated. SVG preview created.')
