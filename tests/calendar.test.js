const fs=require('fs');
const src=fs.readFileSync('qml/views/CalendarUtil.js','utf8').replace('.pragma library','');
const CU=new Function(src+';return {moveCursor,moveSlots,defaultMin,eventsAtCursor,accountsOf,nextAccount,filterAccount,joinUrl,joinLabel,responseLabel,normalise}')();
const eq=(a,b,m)=>{ if(JSON.stringify(a)!==JSON.stringify(b)){console.error('FAIL',m,JSON.stringify(a),JSON.stringify(b));process.exit(1)} };
const D=(y,m,d)=>new Date(y,m-1,d);
const at=(c)=>[c.date.getFullYear(),c.date.getMonth()+1,c.date.getDate(),c.min];

// month: h/l day, j/k week
eq(at(CU.moveCursor({date:D(2026,10,25),min:540},'month',1,0)),[2026,10,26,540],'month l');
eq(at(CU.moveCursor({date:D(2026,10,25),min:540},'month',0,1)),[2026,11,1,540],'month j crosses month');
eq(at(CU.moveCursor({date:D(2026,3,2),min:540},'month',0,-1)),[2026,2,23,540],'month k');
// grids: j/k 30 minutes, wrap over midnight
eq(at(CU.moveCursor({date:D(2026,10,25),min:540},'day',0,1)),[2026,10,25,570],'day j');
eq(at(CU.moveCursor({date:D(2026,10,25),min:0},'week',0,-1)),[2026,10,24,1410],'wrap back');
eq(at(CU.moveCursor({date:D(2026,10,25),min:1410},'week',0,1)),[2026,10,26,0],'wrap fwd');
// work week skips the weekend (2026-10-23 is a Friday)
eq(at(CU.moveCursor({date:D(2026,10,23),min:540},'workweek',1,0)),[2026,10,26,540],'skip weekend fwd');
eq(at(CU.moveCursor({date:D(2026,10,26),min:540},'workweek',-1,0)),[2026,10,23,540],'skip weekend back');
eq(at(CU.moveCursor({date:D(2026,10,24),min:540},'week',1,0)),[2026,10,25,540],'week keeps weekend');
// half page clamps
eq(CU.moveSlots({date:D(2026,10,25),min:1380},10).min,1410,'clamp bottom');
eq(CU.moveSlots({date:D(2026,10,25),min:60},-10).min,0,'clamp top');
eq(CU.defaultMin(D(2026,10,25),new Date(2026,9,25,14,47)),870,'default is current half hour today');
eq(CU.defaultMin(D(2026,10,26),new Date(2026,9,25,14,47)),540,'default 09:00 other days');

// events at cursor
const norm=CU.normalise([
 {id:'a',subject:'Allday',start:'2026-10-25T00:00:00',end:'2026-10-26T00:00:00',is_all_day:true},
 {id:'b',subject:'Nine',start:'2026-10-25T09:00:00',end:'2026-10-25T10:00:00'},
 {id:'c',subject:'Nine30',start:'2026-10-25T09:30:00',end:'2026-10-25T10:00:00'},
 {id:'d',subject:'Other day',start:'2026-10-26T09:00:00',end:'2026-10-26T10:00:00'}]);
const ids=(l)=>l.map(e=>e.raw.id);
eq(ids(CU.eventsAtCursor(norm,D(2026,10,25),0,'month')),['a','b','c'],'month shows all');
eq(ids(CU.eventsAtCursor(norm,D(2026,10,25),540,'day')),['a','b'],'slot 09:00');
eq(ids(CU.eventsAtCursor(norm,D(2026,10,25),570,'day')),['a','b','c'],'slot 09:30');
eq(ids(CU.eventsAtCursor(norm,D(2026,10,25),600,'day')),['a'],'slot 10:00 only all-day');

// accounts
const raw=[{account_email:'z@x.com',account_id:'g-1'},{account_email:'a@x.com',account_id:'e-1'},{account_email:'a@x.com',account_id:'e-1'},{subject:'no account'}];
const acc=CU.accountsOf(raw);
eq(acc.map(a=>a.email),['a@x.com','z@x.com'],'accounts sorted + deduped');
eq(CU.nextAccount(acc,''),'a@x.com','all -> first');
eq(CU.nextAccount(acc,'a@x.com'),'z@x.com','first -> second');
eq(CU.nextAccount(acc,'z@x.com'),'','last -> all');
eq(CU.nextAccount(acc,'gone@x.com'),'','unknown -> all');
eq(CU.nextAccount([],''),'','no accounts');
eq(CU.filterAccount(raw,'a@x.com').length,2,'filter');
eq(CU.filterAccount(raw,'').length,4,'no filter');

// join + labels
eq(CU.joinUrl({join_url:'https://zoom.us/j/1'}),'https://zoom.us/j/1','https ok');
eq(CU.joinUrl({join_url:'http://zoom.us/j/1'}),'','http refused');
eq(CU.joinUrl({join_url:'javascript:alert(1)'}),'','script refused');
eq(CU.joinUrl({}),'','none');
eq(CU.joinLabel({join_provider:'teams'}),'Teams','label');
eq(CU.joinLabel({join_provider:'weird'}),'Link','unknown label');
eq(CU.responseLabel('tentativelyAccepted'),'Tentative','response');
console.log('calendar tests ok');
