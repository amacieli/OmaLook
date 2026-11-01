// node tests/pick.test.js
const fs = require("fs"), vm = require("vm"), assert = require("assert")
const src = fs.readFileSync(__dirname + "/../qml/compose/pick.js", "utf8").replace(".pragma library", "")
const ctx = {}; vm.createContext(ctx); vm.runInContext(src, ctx)
const P = ctx

assert.ok(P.matches("*.pdf", "Report.PDF"))
assert.ok(!P.matches("*.pdf", "report.pdf.bak"))
assert.ok(P.matches("2026-*.png", "2026-10-01 shot.png"))
assert.ok(!P.matches("2026-*.png", "x2026-1.png"))
assert.ok(P.matches("a?c.txt", "abc.txt") && !P.matches("a?c.txt", "ac.txt"))
assert.ok(P.matches("(1).*", "(1).txt"), "regex metacharacters are literal")
assert.ok(!P.matches("a.c*", "abc"), "dot is literal")
assert.ok(P.matches("inv", "My Invoice.pdf") && !P.matches("zzz", "My Invoice.pdf"))
assert.ok(P.matches("", "anything"))

const e = [
  { name: "b.txt", dir: false, size: 5, mtime: 10 },
  { name: "Zdir", dir: true, size: 0, mtime: 1 },
  { name: "a.txt", dir: false, size: 50, mtime: 5 },
  { name: "c.txt", dir: false, size: 1, mtime: 99 },
]
const names = (k) => P.sorted(e, k).map(x => x.name).join(",")
assert.strictEqual(names("name"), "Zdir,a.txt,b.txt,c.txt")
assert.strictEqual(names("newest"), "Zdir,c.txt,b.txt,a.txt")
assert.strictEqual(names("size"), "Zdir,a.txt,b.txt,c.txt")
assert.strictEqual(e[0].name, "b.txt", "input not mutated")
assert.strictEqual(P.nextSort("name"), "newest"); assert.strictEqual(P.nextSort("size"), "name")

const t = P.totals({ "/x/a": { size: 10 }, "/y/b": { size: 5 } })
assert.strictEqual(t.count, 2); assert.strictEqual(t.bytes, 15)
assert.ok(P.over(100, 90, 11) && !P.over(100, 90, 10) && !P.over(0, 1e9, 1e9))

assert.strictEqual(P.shortDir("/home/a/Docs", "/home/a"), "~/Docs")
assert.strictEqual(P.shortDir("/home/ab", "/home/a"), "/home/ab")
const pl = P.places("/home/a", ["/mnt/x", "/home/a/Downloads", "/mnt/x", "/mnt/y"], ["/home/a/Downloads"])
assert.strictEqual(pl.map(x => x.path).join(","), ["/home/a", "/home/a/Downloads", "/mnt/x", "/mnt/y"].join(","))
console.log("pick tests passed")
