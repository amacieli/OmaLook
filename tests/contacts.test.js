const fs=require('fs');
const src=fs.readFileSync('qml/people/fields.js','utf8').replace('.pragma library','');
const F=new Function(src+';return {displayName,oneLine,manyLines,birthdayText,sections,targets,openableUrl,mapUrl,asText,vcard}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,x,y);process.exit(1)} };
eq(F.displayName({displayName:'Ada L'}),'Ada L','display');
eq(F.displayName({givenName:'Ada',surname:'L'}),'Ada L','parts');
eq(F.displayName({emailAddresses:[{address:'a@x.org'}]}),'a@x.org','email only');
eq(F.displayName({companyName:'Acme'}),'Acme','company');
eq(F.displayName({}),'(no name)','nothing');
eq(F.oneLine({street:'1 Main\nApt 2',city:'London',state:'',postalCode:'N1',countryOrRegion:'UK'}),'1 Main, Apt 2, London, N1, UK','one line');
eq(F.manyLines({street:'1 Main',city:'Paris',state:'IDF',postalCode:'75001',countryOrRegion:'FR'}),'1 Main\nParis, IDF 75001\nFR','many lines');
eq(F.manyLines({}),'','empty address');
eq(F.birthdayText('1980-03-03T11:59:00Z'),'3 Mar 1980','birthday');
eq(F.birthdayText('1604-03-03T11:59:00Z'),'3 Mar','no year');
eq(F.birthdayText(null),'','no birthday');
const c={displayName:'Ada',givenName:'Ada',surname:'L',emailAddresses:[{name:'Ada',address:'a@x.org'},{name:'Home',address:'a@h.org'}],
 businessPhones:['1'],homePhones:['2','3'],mobilePhone:'4',homeAddress:{city:'Paris'},companyName:'Acme',birthday:'1980-03-03T11:59:00Z',
 personalNotes:'hello',categories:['OL: Family'],businessHomePage:'acme.test',children:['K1','K2']};
const secs=F.sections(c); eq(secs.map(s=>s.title),['Name','Email','Phone','Address','Work','Personal','Web','Categories','Notes'],'sections present, empty ones left out');
eq(secs[1].rows[1].value,'Home <a@h.org>','named email');
eq(secs[1].rows[0].value,'Ada <a@x.org>','named email');
eq(F.sections({emailAddresses:[{name:'a@x.org',address:'a@x.org'}]})[0].rows[0].value,'a@x.org','name equal to address is not repeated');
eq(secs[2].rows.map(r=>r.label+':'+r.number),['Business:1','Home:2','Home:3','Mobile:4'],'phones');
eq(F.targets(c,['email']).map(t=>t.value),['a@x.org','a@h.org'],'email targets');
eq(F.targets(c,['phone']).length,4,'phone targets');
eq(F.targets(c,['address'])[0].value,'Paris','address target');
eq(F.targets(c,['url'])[0].value,'acme.test','url target');
eq(F.openableUrl('acme.test'),'https://acme.test','bare url');
eq(F.openableUrl('http://x.y'),'http://x.y','full url');
eq(F.openableUrl(''),'','no url');
eq(F.mapUrl('1 A St, Paris'),'https://www.openstreetmap.org/search?query=1%20A%20St%2C%20Paris','map');
if(!F.asText(c).includes('Birthday: 3 Mar 1980')) {console.error('FAIL text');process.exit(1)}
const v=F.vcard(c); if(!v.startsWith('BEGIN:VCARD')||!v.endsWith('END:VCARD')||!v.includes('EMAIL;TYPE=INTERNET:a@x.org')||!v.includes('TEL;TYPE=CELL:4')||!v.includes('BDAY:1980-03-03')) {console.error('FAIL vcard',v);process.exit(1)}
eq(F.sections({}),[], 'empty contact');
console.log('ok contacts');
