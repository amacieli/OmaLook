const fs=require('fs');
const src=fs.readFileSync('qml/mail/format.js','utf8').replace('.pragma library','');
const X=new Function(src+';return {sanitizeHtml,_cssLoads}')();
const eq=(a,b,m)=>{ if(a!==b){console.error('FAIL',m,JSON.stringify(a),JSON.stringify(b));process.exit(1)} };
const san=(h,allow)=>X.sanitizeHtml(h,600,!!allow).html;
const noStyleUrl=(style,m)=>{
  const out=san('<div style="'+style+'">x</div>');
  if(/url|style/i.test(out)) { console.error('FAIL',m,out); process.exit(1) }
  eq(out,'<div>x</div>',m);
};
// style url() variants: all removed
[
 'background:url(http://evil.test/a.gif)',
 'background:URL(http://evil.test/a.gif)',
 'background: URL (http://evil.test/a.gif)',
 'background:u\\rl(http://evil.test/a.gif)',
 'background:u\\72 l(http://evil.test/a.gif)',
 'background:\\75 rl(http://evil.test/a.gif)',
 'background:\\000075rl(http://evil.test/a.gif)',
 'background:u/**/rl(http://evil.test/a.gif)',
 'background:u&#x09;rl(http://evil.test/a.gif)',
 'background:&#117;rl(http://evil.test/a.gif)',
 'background-image:U\tR\nL(http://evil.test/a.gif)',
 'list-style-image:url(x.png)',
 "background-image:image-set('http://evil.test/a.png' 1x)",
 "color:red;@import 'http://evil.test/x.css'",
].forEach((st,i)=>noStyleUrl(st,'style variant '+i));
// keeps harmless style
eq(san('<p style="color:#333;font-weight:bold">x</p>'),'<p style="color:#333;font-weight:bold">x</p>','plain style kept');
// style with > inside a quoted value does not leak a url
const gt=san('<td style="content:\'>\';background:url(http://evil.test/g.gif)">x</td>');
if(/url|evil/i.test(gt)) { console.error('FAIL gt style leaks', gt); process.exit(1) }
// hrefs: non-web schemes dropped, text kept
const bad=['javascript:alert(1)','JAVASCRIPT:alert(1)','java&#x09;script:alert(1)','java&#10;script:x','data:text/html,<b>x</b>',
 'file:///etc/passwd','vbscript:msgbox(1)','ftp://x.org/a','/relative','#top','zoommtg://x'];
bad.forEach((h,i)=>{
  const out=san('<a href="'+h+'">click me</a>');
  if(/href/i.test(out)) { console.error('FAIL href kept',h,out); process.exit(1) }
  eq(out,'<a>click me</a>','href dropped, text kept #'+i);
});
const unq=san('<a href=javascript:alert(1)>unq</a>');
eq(unq,'<a>unq</a>','unquoted javascript href dropped');
// allowed hrefs kept
['https://example.com/a?b=1&amp;c=2','http://example.com','mailto:bob@x.org?subject=hi','HTTPS://Example.com/Caps'].forEach((h,i)=>{
  const out=san('<a href="'+h+'">ok</a>');
  if(out.indexOf('href="'+h+'"')<0) { console.error('FAIL allowed href dropped',h,out); process.exit(1) }
});
// remote images still never reach Qt, even as style or href
const img=san('<img src="https://evil.test/p.gif" alt="pix">');
if(/https?:/i.test(img)) { console.error('FAIL remote img leaked',img); process.exit(1) }
console.log('ok sanitize');
