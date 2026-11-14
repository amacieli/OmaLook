const fs=require('fs');
const src=fs.readFileSync('qml/compose/attach.js','utf8').replace('.pragma library','');
const A=new Function(src+';return {matchRegistered}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,x,y);process.exit(1)} };

// all accepted: the handle from the backend, the display name from the pick, the path as typed kept locally
let r=A.matchRegistered([{path:'/a/link.txt',name:'link.txt',size:0},{path:'/b/pic.png',name:'pic.png',size:0}],
  [{handle:'fh-1',name:'x.txt',size:5},{handle:'fh-2',name:'pic.png',size:9}],[]);
eq(r.entries,[{handle:'fh-1',name:'link.txt',size:5,localPath:'/a/link.txt'},{handle:'fh-2',name:'pic.png',size:9,localPath:'/b/pic.png'}],'all accepted');
eq(r.refusedMsg,'','no refusal message');

// one refused: skipped, message names it
r=A.matchRegistered([{path:'/a/bad',name:'bad',size:0},{path:'/a/ok.txt',name:'ok.txt',size:0}],
  [{handle:'fh-3',name:'ok.txt',size:3}],[{path:'/a/bad',error:'outside the allowed folders'}]);
eq(r.entries,[{handle:'fh-3',name:'ok.txt',size:3,localPath:'/a/ok.txt'}],'refused skipped');
eq(r.refusedMsg,'Not attached: bad (outside the allowed folders)','refused message');

// everything refused
r=A.matchRegistered([{path:'~/s',name:'s',size:0}],[],[{path:'~/s',error:'no'}]);
eq([r.entries,r.refusedMsg],[[],'Not attached: s (no)'],'all refused');

// counts disagree: match by base name
r=A.matchRegistered([{path:'/a/one',name:'one',size:0},{path:'/a/two',name:'two',size:0}],
  [{handle:'fh-4',name:'two',size:2}],[]);
eq(r.entries,[{handle:'fh-4',name:'two',size:2,localPath:'/a/two'}],'fallback by name');
eq(r.refusedMsg,'Not attached: one (not registered)','unmatched by name');
console.log('PASS attach_register');
