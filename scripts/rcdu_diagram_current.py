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

text(40,20,'rcdu | live filesystem architecture',32)
text(40,66,'Current implementation after compatibility cleanup | Linux / Rust | 2026-10-08',16,'#868e96')
panel(110,450,'01  Entry point, session and results','Solid arrows: execution / data flow. Dashed arrows: control / feedback. Each box identifies its source module.')
box('cli',70,225,350,140,'main.rs -> session::run\ncli.rs: parse arguments\n/etc/rcdu.conf + user rcdu/config\nCommand-line values override config\nHelp / version return immediately')
box('session',480,225,390,140,'session.rs owns application lifetime\nSignals guard + ncurses progress UI\nLive root directory only\nscan::scan_with_progress\nUI modes: none / line / curses')
box('model',950,225,420,140,'scan.rs -> accounted Model\nWorker-owned Parts joined\nmodel.rs: Entries / Directories\nDirectory spans index children\nAllocated + apparent + shared totals','green')
box('browser',1460,225,450,140,'browser.rs + display.rs\nBrowse directories and file sizes\nSort / navigate / info / help / toggles\nD/d: delete  |  r: rescan  |  q: quit\nCurses draws and reads keys','orange')
arrow([(420,290),(480,290)])
arrow([(870,290),(950,290)])
arrow([(1370,290),(1460,290)])
box('headless',950,420,420,100,'--quit-after-scan\nPrint root allocated / apparent bytes\nand item count as plain text; then exit','gray')
arrow([(1160,365),(1160,420)])
text(70,425,'os/linux/curses.rs: one terminal owner.\nSignal / q / terminal error -> cancellation.\nGuards restore terminal state on exit.',16)
text(1460,425,'No tree import/export or format modules.\nNo ncdu config or environment hooks.\nlibc + Unicode crates; ncursesw via FFI.',16)
panel(590,600,'02  Scan scheduling, storage and accounting','scan.rs shares directory jobs and metadata jobs, so multiple workers can process a single wide directory.')
box('root',70,710,320,145,'Open + verify root descriptor\n-t 0: available CPU count\n-t N: explicit worker count\nDescriptor budget limits pool\nos/linux.rs','blue')
box('queue',440,710,320,145,'Shared bounded queue\nMutex + Condvar\nDirectory ID jobs\nMetadata batch jobs\nTrack outstanding jobs','blue')
box('enum',810,710,320,145,'A worker enumerates directory\ngetdents64: 32 KiB buffer\nUp to 256 names per batch\nShared parent descriptor\nNames remain raw basename bytes','green')
box('observe',1180,710,340,145,'Any worker processes metadata\nfstatat + exclude.rs rules\nFilesystem / cache / kernel rules\nCreate child directory IDs\nOrdinary subtotals at observation','green')
box('parts',1570,710,340,145,'Worker appends to its own Part\nstorage.rs: fixed growth chunks\n24-byte Entry; 4096-entry chunk\nRaw names in 64 KiB chunks\nSpans + hardlink / extended data','green')
for a,b in [(390,440),(760,810),(1130,1180),(1520,1570)]: arrow([(a,780),(b,780)])
arrow([(965,710),(965,675),(600,675),(600,710)],'publish metadata batches',(660,682),True)
arrow([(1260,855),(1260,900),(590,900),(590,855)],'publish discovered directories',(740,911),True)
text(70,970,'When queue is full:\nMetadata runs locally. Directory\noverflow retains IDs only and is\nredistributed as capacity returns.\nNo recursive worker call stack.',16)
box('account',810,1000,710,135,'Join worker Parts without copying every file\nmodel::finish_accounting()\n1. Reverse directory reduction of ordinary totals + errors\n2. Hardlinks grouped by (device, inode): once per ancestor\nIncludes directory own size; shared usage; saturating sums','blue')
arrow([(1740,855),(1740,1070),(1520,1070)])
text(1570,1115,'Fully accounted Model -> session',15,'#2b8a3e')
panel(1220,440,'03  Real deletion and refresh','browser.rs uses the observed model and verified directory descriptors. Deleted entries are removed from disk.')
box('actions',70,1350,350,145,'Browser input\nD/d: selected file or directory\nDefault confirmation: y / N\nProgress: q stops; errors: a/i/I\nr: rescan; -r disables deletion','orange')
box('rescan',500,1330,410,110,'Refresh: verify current directory\nscan::scan_descriptor -> subtree Model\nProgress + cancellation on UI thread','green')
box('delete',500,1490,410,110,'delete.rs: postorder observed entries\nos/linux.rs: verify + unlinkat\nSymlink flag: unlink link, not target\nTrack removed IDs / errors / abort','orange')
box('rebuild',1020,1380,410,130,'mutation::rebuild\nReplace subtree OR omit removed IDs\nUpdate surviving hardlink counts\nBuilder::finish recalculates totals','green')
box('install',1520,1380,390,130,'Browser::install_model\nRestore location by path\nRestore selected name if it survives\nReload and draw updated sizes','orange')
arrow([(420,1370),(500,1370)])
arrow([(420,1450),(460,1450),(460,1545),(500,1545)])
arrow([(910,1385),(970,1385),(970,1410),(1020,1410)])
arrow([(910,1545),(970,1545),(970,1470),(1020,1470)])
arrow([(1430,1445),(1520,1445)])
text(1020,1550,'Partial deletion: rebuild successful removals and report failures.\nRefresh incomplete non-cancelled cases; r rescans on demand.\nOptional b shell uses $SHELL; curses suspends and resumes.',14)
text(40,1700,'Verification: scanner/config/display/mutation Rust tests plus disposable CLI and PTY fixtures for D file/directory deletion.',14,'#868e96')
scene={'type':'excalidraw','version':2,'source':'https://excalidraw.com','elements':elements,'appState':{'viewBackgroundColor':'#f8f9fa','gridSize':None,'scrollX':0,'scrollY':0,'zoom':{'value':0.65}},'files':{}}
(out/'rcdu-architecture.excalidraw.json').write_text(json.dumps(scene,indent=2)+'\n')
header='<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="1750" viewBox="0 0 2000 1750"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="#868e96"/></marker></defs><rect width="2000" height="1750" fill="#f8f9fa"/>'
(out/'rcdu-architecture.svg').write_text(header+'\n'.join(svg)+'</svg>\n')
ids={e['id']:e for e in elements}
assert len(ids)==len(elements)
for e in elements:
 if e.get('containerId'): assert e['containerId'] in ids
 for bound in e.get('boundElements') or []: assert bound['id'] in ids
print(f'Created current architecture JSON: {len(elements)} elements; references validated. SVG preview created.')
