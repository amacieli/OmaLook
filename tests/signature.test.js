const fs=require('fs');
const src=fs.readFileSync('qml/compose/signature.js','utf8').replace('.pragma library','');
const S=new Function(src+';return {block,insertion,locate,quoteStart}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,x,y);process.exit(1)} };
eq(S.block('Adam\nCTO',true),'-- \nAdam\nCTO','system: RFC 3676 delimiter with trailing space');
eq(S.block('Adam',false),'Adam','html: no delimiter');
eq(S.block('   ',true),'','blank signature inserts nothing');
eq(S.block(null,false),'','null signature');
eq(S.insertion('x',null,true,-1),null,'no signature, no insertion');

// reply: block goes at the quote start, with a blank line after it
const plain='\nOn Mon, Ann wrote:\n> hi';
const q=S.quoteStart(plain,'On Mon, Ann wrote:');
eq(q,1,'quote start index');
const ins=S.insertion(plain,'Adam',true,q);
eq(ins,{at:1,text:'-- \nAdam\n\n'},'reply insertion above quote');
const after=plain.slice(0,ins.at)+ins.text+plain.slice(ins.at);
eq(after,'\n-- \nAdam\n\nOn Mon, Ann wrote:\n> hi','reply result');
// the user typed above the block (cursor at 0): the block is still found and removable
const typed='thanks\n'+after.slice(1);
const pos=S.locate(typed,ins.text);
eq(pos,'thanks\n'.length-1+1,'block still located after typing above');
eq(typed.slice(0,pos)+typed.slice(pos+ins.text.length),'thanks\nOn Mon, Ann wrote:\n> hi','removal leaves the reply text');

// new message: empty editor, block on the line below the cursor
const ni=S.insertion('',"Adam",false,-1);
eq(ni,{at:0,text:'\nAdam\n'},'new message insertion');
eq(S.locate('hello\nAdam\n','\nAdam\n'),5,'new message block located after typing');

// the user edited the block: it is not found, so it is left alone
eq(S.locate('\n-- \nAdam (edited)\n\nOn','\n-- \nAdam\n\n'),-1,'edited block not removed');
eq(S.locate('anything',null),-1,'nothing inserted');
eq(S.quoteStart('no quote here','On x wrote:'),-1,'no quote line');
eq(S.quoteStart('abc',null),-1,'no quote text');
console.log('ok signature');
