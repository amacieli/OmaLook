const fs=require('fs');
const src=fs.readFileSync('qml/mail/bounce.js','utf8').replace('.pragma library','');
const B=new Function(src+';return {parse,notice,message}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,x,y);process.exit(1)} };
eq(B.parse('a@x.org, Bob <b@y.org>; "Q, R" <c@z.org>, A@X.org, nope, d@e'),
   {to:[{email:'a@x.org',name:''},{email:'b@y.org',name:'Bob'},{email:'c@z.org',name:'Q, R'}],bad:['nope','d@e']},'parse');
eq(B.parse('').to,[],'empty'); eq(B.parse(' , ;').bad,[],'blank tokens');
const info={subject:'Quarterly',sender:'Bob <bob@o.org>',replyTo:'',account:'me@x.org',delaySecs:3,accountId:'gmail-1',messageId:'gmail-1:abc'};
const to=[{email:'n@y.org',name:''}];
const n=B.notice(info,to);
for (const s of ['Bounce \u201cQuarterly\u201d to n@y.org?','from YOU (me@x.org), not from Bob <bob@o.org>','not a true bounce','replies go to Bob <bob@o.org>','3 s; u takes it back']) if(!n.includes(s)){console.error('FAIL notice lacks',s,'\n'+n);process.exit(1)}
if(!B.notice({...info,replyTo:'list@o.org',delaySecs:0},to).includes('replies go to list@o.org')) {console.error('FAIL replyTo');process.exit(1)}
if(!B.notice({...info,delaySecs:0},to).includes('at once')) {console.error('FAIL delay0');process.exit(1)}
const m=B.message(info,to);
eq([m.kind,m.account_id,m.in_reply_to,m.to,m.cc,m.bcc,m.attachments],['bounce','gmail-1','gmail-1:abc',[{email:'n@y.org',name:''}],[],[],[]],'message');
console.log('ok');
