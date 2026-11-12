const fs=require('fs');
const src=fs.readFileSync('qml/compose/attach.js','utf8').replace('.pragma library','');
const A=new Function(src+';return {layout,counted,capacity,meter,states,displayNames,combine,defaultLimits,encodedPart,clipboardFileType,clipboardHasPng,isPicture,pathsFromClipboard,baseName,dirName,pictureFiles}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,x,y);process.exit(1)} };
const MB=1048576;

// which parts of the strip exist for N files
eq(A.layout(0),{show:false,chips:0,more:0},'0 files: no strip');
eq(A.layout(1),{show:true,chips:1,more:0},'1 file');
eq(A.layout(3),{show:true,chips:3,more:0},'3 files: all chips');
eq(A.layout(4),{show:true,chips:2,more:2},'4 files: two chips and +2');
eq(A.layout(40),{show:true,chips:2,more:38},'40 files: two chips and +38');

// limits per provider (same numbers as src/compose.rs)
const g=A.defaultLimits('gmail'), x=A.defaultLimits('exchange');
eq([g.hard,g.warn,g.basis],[36700160,25000000,'encoded'],'gmail limits');
eq([x.hard,x.warn,x.basis],[150*MB,null,'raw'],'graph limits');

// the estimate never undercounts what compose.rs counts (encoded_part_size: ceil(raw/3)*4 + CRLFs + 400)
eq(A.encodedPart(0),400,'empty file');
eq(A.encodedPart(57),76+2+400,'one full line');
eq(A.encodedPart(58),80+2+2+400,'one line and a bit');

// Graph counts raw bytes
eq(A.meter(x,[{size:100*MB},{size:50*MB}]).level,'warn','150 MB is at the limit: warn, not over');
eq(A.meter(x,[{size:100*MB},{size:51*MB}]).level,'over','151 MB is over for Graph');
eq(A.meter(x,[{size:10*MB}]).level,'ok','10 MB is fine for Graph');
eq(A.meter(x,[]).level,'ok','nothing attached');

// Gmail counts the encoded message: 24 MB of files is over the soft limit but allowed, 27 MB is refused
eq(A.meter(g,[{size:10*MB}]).level,'ok','10 MB of files on Gmail');
eq(A.meter(g,[{size:24000000}]).level,'warn','24 MB of files on Gmail: warn');
eq(A.meter(g,[{size:27*MB}]).level,'over','27 MiB of files on Gmail: over');
eq(A.meter(g,[{size:26500000}]).level,'warn','26.5 MB still fits (as in the daemon test)');
eq(A.meter(g,[{size:26900000}]).level,'over','26.9 MB does not (as in the daemon test)');

// the "of y MB" figure for Gmail is the encoded limit shrunk by base64 growth
const cap=A.capacity(g);
eq(cap>26000000 && cap<27000000,true,'gmail capacity near 26.5 MB: '+cap);
eq(A.capacity(x),150*MB,'graph capacity');

// switching From re-evaluates: the same files are fine on Graph and over on Gmail, and one chip turns red
const files=[{name:'small.txt',size:1000},{name:'huge.zip',size:27*MB}];
eq(A.states(x,files).state,['ok','ok'],'fine on Graph');
const sg=A.states(g,files);
eq(sg.state,['ok','big'],'the biggest file turns red on Gmail');
eq(/too big for Gmail/.test(sg.reason[1]),true,'reason names Gmail: '+sg.reason[1]);
eq(sg.reason[0],'','no reason on a fine file');

// duplicate names are told apart in the strip only
eq(A.displayNames([{name:'scan.pdf'},{name:'b.txt'},{name:'Scan.pdf'},{name:'scan.pdf'}]),['scan.pdf','b.txt','Scan.pdf (2)','scan.pdf (3)'],'duplicates');

// originals first, then picked files, each remembering its own index
const rows=A.combine([{name:'o1',size:1},{name:'o2',size:2}],[{name:'p1',size:3}]);
eq(rows.map(r=>r.origin+':'+r.ref),['original:0','original:1','picked:0'],'combined order');

// clipboard files
eq(A.clipboardFileType('text/plain\ntext/uri-list\n'),'text/uri-list','uri-list wins');
eq(A.clipboardFileType('x-special/gnome-copied-files\nSTRING\n'),'x-special/gnome-copied-files','gnome');
eq(A.clipboardFileType('text/plain\ntext/html\n'),'','plain text is not files');
eq(A.clipboardFileType(''),'','empty');
eq(A.pathsFromClipboard('copy\nfile:///home/a/My%20Doc.pdf\nfile:///tmp/x.png\n'),['/home/a/My Doc.pdf','/tmp/x.png'],'gnome body');
eq(A.pathsFromClipboard('# c\r\nfile://localhost/tmp/a.txt\r\nhttps://x.org/y\r\nfile:///tmp/a.txt\r\nfile:///tmp/%E2%82%AC.txt'),['/tmp/a.txt','/tmp/\u20ac.txt'],'uri-list: comments, remote, duplicates, utf-8');
eq(A.pathsFromClipboard('file:///tmp/%ZZ'),[],'bad escape ignored');
eq([A.baseName('/a/b/c.txt'),A.dirName('/a/b/c.txt'),A.dirName('/c.txt')],['c.txt','/a/b','/'],'names');
// Inline pictures (OL-054): which picks go into the text, and which clipboards hold PNG data.
eq([A.isPicture('a.PNG'),A.isPicture('p.jpeg'),A.isPicture('x.webp'),A.isPicture('d.svg'),A.isPicture('n.txt')],[true,true,true,false,false],'picture names');
eq([A.clipboardHasPng('text/plain\nimage/png\n'),A.clipboardHasPng('text/plain'),A.clipboardHasPng('image/jpeg')],[true,false,false],'clipboard png');
console.log('ok attach');

// OL-054 system-font switch: pictures still in the text become files; a picture already deleted from the
// text is not turned into a file (it used to reappear in the strip as a duplicate), and a path listed twice is kept once.
{
  const pic=(p,c)=>({path:p,name:p.split('/').pop(),size:5,inline_cid:c});
  const file={path:'/d/notes.txt',name:'notes.txt',size:2};
  let r=A.pictureFiles([pic('/p/shot-1.png','c1'),file],['c1']);
  eq(r.converted,1,'one live picture converted');
  eq(r.files,[{path:'/p/shot-1.png',name:'shot-1.png',size:5},file],'picture becomes a plain file, normal file kept');
  r=A.pictureFiles([pic('/p/shot-1.png','gone'),pic('/p/clip.png','c2')],['c2']);
  eq(r.converted,1,'deleted-from-text entry not counted');
  eq(r.files.map(a=>a.name),['clip.png'],'deleted-from-text entry dropped, not turned into a file');
  r=A.pictureFiles([pic('/p/shot-1.png','c1'),pic('/p/shot-1.png','c3')],['c1','c3']);
  eq(r.files.length,1,'same file twice in the text: one attachment');
  eq(r.converted,2,'both pictures still counted as converted');
  eq(A.pictureFiles([],[]),{files:[],converted:0},'nothing attached');
}
