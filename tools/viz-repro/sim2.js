const lib = require('./pkg/viz_repro.js');
function engineLine(jsval) {
  return (typeof jsval === 'string') ? jsval : JSON.stringify(jsval) + "\n";
}
function roomViewParse(buffer) {
  const drawn = [];
  buffer.split("\n").forEach(function(e) {
    if (e) { e = JSON.parse(e); drawn.push(e.t); }
  });
  return drawn;
}
function isString(v){return typeof v==='string';}
function isNumber(v){return typeof v==='number';}
function vp(n,x,y){return isString(n)&&isNumber(x)&&isNumber(y);}
function validate(o){
  switch(o.t){
    case 'l': return vp(o.n1,o.x1,o.y1)&&vp(o.n2,o.x2,o.y2);
    case 'c': return vp(o.n,o.x,o.y);
    case 'p': return Array.isArray(o.points)&&!o.points.some(({n,x,y})=>!vp(n,x,y));
    case 'r': return vp(o.n,o.x,o.y)&&isNumber(o.w)&&isNumber(o.h);
    case 't': return vp(o.n,o.x,o.y);
    default: return false;
  }
}
function mapViewParse(buffer) {
  return buffer.split('\n').filter(_=>!!_).map(i=>JSON.parse(i)).filter(validate);
}

console.log("=== VECTOR 1: string addVisual WITHOUT trailing newline, then another visual ===");
const stringNoNl = '{"t":"c","x":25,"y":25,"n":"W5N5","s":{}}'; // engine appends AS-IS
let buf = engineLine(stringNoNl) + engineLine(lib.map_circle());
console.log("buffer:", JSON.stringify(buf.slice(0,120)));
try { console.log("map view ok:", mapViewParse(buf).length); } catch(e){ console.log("MAP VIEW DIED:", e.message); }

console.log("\n=== VECTOR 1b: '' global buffer ends without newline; backend concat with room buffer ===");
const globalBuf = '{"t":"c","x":25,"y":25,"s":{}}'; // string write, forgot \n
const roomBuf = engineLine(lib.room_circle()) + engineLine(lib.room_poly());
const combined = "" + globalBuf + roomBuf;
console.log("joint:", JSON.stringify(combined.slice(0,90)));
try { console.log("room view drew:", roomViewParse(combined).join(',')); } catch(e){ console.log("ROOM VIEW DIED:", e.message, "→ canvas stays cleared"); }

console.log("\n=== VECTOR 2: map text without s — passes validate, drawText derefs obj.s ===");
const sless = '{"t":"t","text":"X","x":25,"y":25,"n":"W5N5"}\n';
const objs = mapViewParse(sless + engineLine(lib.map_circle()));
console.log("validated objects:", objs.map(o=>o.t).join(','));
try {
  for (const o of objs) { if (o.t==='t') { const c = o.s.color; } }
  console.log("drawText ok");
} catch(e){ console.log("drawText THREW:", e.message, "→ VisualLayer.draw aborts AFTER clear() → all map visuals blank + rxjs subscription dies"); }

console.log("\n=== VECTOR 3: NaN/Infinity coords (map) ===");
const nanLine = '{"t":"c","x":null,"y":25,"n":"W5N5","s":{}}\n';
const r = mapViewParse(nanLine + engineLine(lib.map_circle()));
console.log("survivors:", r.length, "(bad object filtered per-object — tolerant)");

console.log("\n=== VECTOR 4: ES Map under JSON.stringify (serde maps) ===");
console.log("JSON.stringify(new Map([['t','c']])) =", JSON.stringify(new Map([['t','c']])));
