const fs=require('fs');
const src=fs.readFileSync('qml/compose/external.js','utf8').replace('.pragma library','');
const X=new Function(src+';return {domainOf,isConsumer,bannerDomains,externalRecipients,bannerText}')();
const eq=(a,b,m)=>{ const x=JSON.stringify(a), y=JSON.stringify(b); if(x!==y){console.error('FAIL',m,x,y);process.exit(1)} };
const exo={provider:'graph',email:'Adam@Contoso.com'};
eq(X.domainOf(' A@B.Com '),'b.com','domain lowercased');
eq(X.domainOf('nodomain'),'','no @');
eq(X.bannerDomains(exo),['contoso.com'],'work graph account gets banner');
eq(X.bannerDomains({provider:'graph',email:'me@outlook.com'}),null,'personal microsoft: no banner');
eq(X.bannerDomains({provider:'graph',email:'me@Hotmail.com'}),null,'consumer domain case-insensitive');
eq(X.bannerDomains({provider:'gmail',email:'a@corp.com'}),null,'gmail: no banner');
eq(X.bannerDomains(null),null,'no account');
const inside=['contoso.com'];
eq(X.externalRecipients([{email:'bob@Contoso.com',name:'',valid:true},{email:'x@gmail.com',name:'X',valid:true}],null,inside),
   [{email:'x@gmail.com',name:'X'}],'inside vs consumer');
eq(X.externalRecipients([{email:'a@partner.org',valid:true},{email:'A@partner.org',valid:true}],null,inside),
   [{email:'a@partner.org',name:''}],'dedupe ignoring case');
eq(X.externalRecipients([{email:'bad@@x',valid:false}],null,inside),[],'invalid entries skipped');
eq(X.externalRecipients([{list:'7',name:'Team'},{email:'z@ext.io',valid:true}],
   (id)=> id==='7' ? [{email:'m@contoso.com'},{email:'n@other.net'}] : [], inside),
   [{email:'n@other.net',name:''},{email:'z@ext.io',name:''}],'list members checked, unknown list empty');
eq(X.externalRecipients([{email:'a@b.io',valid:true}],null,null),[],'no inside = no externals');
eq(X.externalRecipients([],null,inside),[],'empty');
const many=['a@x.io','b@x.io','c@x.io','d@x.io','e@x.io'].map(e=>({email:e,valid:true}));
eq(X.bannerText(['contoso.com'],X.externalRecipients(many,null,inside)),
   'External recipients (outside @contoso.com): a@x.io, b@x.io, c@x.io, d@x.io and 1 more','banner text caps list');
console.log('ok external');
