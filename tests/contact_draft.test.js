const fs=require('fs');
const src=fs.readFileSync('qml/people/draft.js','utf8').replace('.pragma library','');
const D=new Function(src+';return {DEFS,defaultIds,valueOf,valuesOf,shownIds,pickable,parseBirthday,validate,buildFields,changeCount,splitList}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,'\n got',x,'\n want',y);process.exit(1)} };
// ids unique
eq(new Set(D.DEFS.map(d=>d.id)).size, D.DEFS.length, 'unique ids');
eq(D.defaultIds(), ['givenName','surname','email0','mobilePhone','companyName','jobTitle','personalNotes'], 'defaults');
const raw={givenName:'Ada',surname:'L',emailAddresses:[{name:'Ada',address:'a@x.org'},{name:'h@x.org',address:'h@x.org'}],
 businessPhones:['1'],homePhones:[],mobilePhone:'4',homeAddress:{street:'1 St',city:'Paris',state:'',postalCode:'',countryOrRegion:'FR'},
 birthday:'1604-03-03T11:59:00Z',children:['K1','K2'],categories:['OL: Family'],personalNotes:'hi'};
const v=D.valuesOf(raw);
eq(v.email0,'a@x.org','email0'); eq(v.email1,'h@x.org','email1'); eq(v.email0name,'Ada','named'); eq(v.email1name,'','name equal address is no name');
eq(v.business0,'1','phone'); eq(v.business1,'',''); eq(v['homeAddress.city'],'Paris','address part'); eq(v.birthday,'03-03','yearless'); eq(v.children,'K1, K2','list');
eq(D.valuesOf({birthday:'1980-03-03T11:59:00Z'}).birthday,'1980-03-03','birthday with year');
const shown=D.shownIds(v);
eq(shown.includes('business0'),true,'filled field shown'); eq(shown.includes('home0'),false,'empty hidden field stays hidden');
eq(['homeAddress.street','homeAddress.city','homeAddress.state','homeAddress.postalCode','homeAddress.countryOrRegion'].every(i=>shown.includes(i)),true,'whole address shown');
eq(shown.includes('businessAddress.city'),false,'other address hidden');
const pick=D.pickable(D.defaultIds());
eq(pick.filter(p=>p.label==='Home address')[0].add.length,5,'address is one pick of five');
eq(pick.some(p=>p.id==='givenName'),false,'shown fields are not offered');
eq(D.pickable(shown).some(p=>p.label==='Home address'),false,'shown address not offered');
// birthday
eq(D.parseBirthday('05-17'),'1604-05-17','no year'); eq(D.parseBirthday('2000-5-7'),'2000-05-07','pad'); eq(D.parseBirthday(''),'','blank');
eq(D.parseBirthday('02-30'),null,'feb 30'); eq(D.parseBirthday('2001-02-29'),null,'not leap'); eq(D.parseBirthday('2000-02-29'),'2000-02-29','leap'); eq(D.parseBirthday('soon'),null,'text');
// validate
eq(D.validate({email0:'nope'}),"'nope' is not an email address",'bad email'); eq(D.validate({email0:'',email0name:'x'}).includes('no address'),true,'name no address');
eq(D.validate({email0:'a@x.org',birthday:'13-40'}).startsWith('Birthday'),true,'bad birthday'); eq(D.validate({email0:'a@x.org',birthday:''}),'','ok');
// buildFields: only touched groups
const w=Object.assign({},v,{surname:'Lovelace',email1:'new@x.org',['homeAddress.city']:'Rome',business1:'2',birthday:'',children:'K1'});
const f=D.buildFields(w,v);
eq(Object.keys(f).sort(),['birthday','businessPhones','children','emailAddresses','homeAddress','surname'],'only touched groups');
eq(f.emailAddresses,[{name:'Ada',address:'a@x.org'},{address:'new@x.org'}],'emails rebuilt'); eq(f.businessPhones,['1','2'],'phones'); eq(f.homeAddress.city,'Rome','address');
eq(f.homeAddress.street,'1 St','whole address sent'); eq(f.birthday,null,'blank birthday clears'); eq(f.children,['K1'],'list');
eq(D.buildFields(v,v),{},'nothing changed, nothing sent');
// new contact
const cr=D.buildFields({givenName:'Bo',email0:'b@x.org',mobilePhone:'5'},{}); eq([cr.givenName,cr.emailAddresses,cr.mobilePhone,Object.keys(cr).length],['Bo',[{address:'b@x.org'}],'5',3],'create');
// clearing a field sends empty
eq(D.buildFields(Object.assign({},v,{mobilePhone:''}),v),{mobilePhone:''},'clear text');
eq(D.buildFields(Object.assign({},v,{email0:'',email0name:''}),v).emailAddresses,[{address:'h@x.org'}],'remove first email');
eq(D.changeCount(w,v),6,'count');
// extended (MAPI) fields
const rx={singleValueExtendedProperties:[{id:'String 0x3a23',value:'555'},{id:'Integer 0x3a4d',value:'1'},{id:'SystemTime 0x3a41',value:'2001-02-03T11:59:00Z'}]};
const vx=D.valuesOf(rx); eq([vx.x_fax,vx.x_gender,vx.x_anniversary,vx.x_pager],['555','female','2001-02-03',''],'ext values');
eq(D.shownIds(vx).includes('x_fax'),true,'filled ext shown'); eq(D.shownIds(vx).includes('x_pager'),false,'empty ext hidden');
const fx=D.buildFields(Object.assign({},vx,{x_fax:'777',x_gender:'Male'}),vx); eq(fx,{extended:{fax:'777',gender:'male'}},'ext changes');
eq(D.validate({x_gender:'robot'}).startsWith('Gender'),true,'gender'); eq(D.validate({x_anniversary:'x'}).startsWith('Anniversary'),true,'anniversary');
console.log('ok draft');
