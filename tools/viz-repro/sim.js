const lib = require('./pkg/viz_repro.js');

// === ENGINE SIDE (console.js addVisual): stringify exactly as engine does ===
function engineLine(jsval) {
  const _ = { isString: v => typeof v === 'string' };
  return _.isString(jsval) ? jsval : JSON.stringify(jsval) + "\n";
}

const cases = {
  map_circle: lib.map_circle(),
  map_text: lib.map_text(),
  map_line: lib.map_line(),
  map_poly: lib.map_poly(),
  map_rect: lib.map_rect(),
  room_circle: lib.room_circle(),
  room_text_nan: lib.room_text_nan(),
  room_poly: lib.room_poly(),
};

console.log("=== raw JsValue types & engine-stringified lines ===");
for (const [k, v] of Object.entries(cases)) {
  const tag = Object.prototype.toString.call(v);
  const line = engineLine(v);
  console.log(`${k}: jsType=${tag} constructor=${v && v.constructor && v.constructor.name}`);
  console.log(`  line: ${JSON.stringify(line)}`);
}

// === CLIENT SIDE 1: room-view canvas directive parse (build.min.js appRoomVisual) ===
function roomViewParse(buffer) {
  const drawn = [];
  buffer.split("\n").forEach(function(e) {
    if (e) {
      e = JSON.parse(e); // no try/catch in client
      drawn.push(e.t);
    }
  });
  return drawn;
}

// === CLIENT SIDE 2: map view VisualService parse + validate ===
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

console.log("\n=== map-target buffer through map-view client ===");
const mapBuf = ['map_circle','map_text','map_line','map_poly','map_rect'].map(k=>engineLine(cases[k])).join('');
try {
  const objs = mapViewParse(mapBuf);
  console.log("parsed+validated:", objs.map(o=>o.t).join(','), `(of 5)`);
  // simulate drawText obj.s dereference
  for (const o of objs) {
    if (o.t === 't') { const c = o.s.color; console.log("drawText s.color ok:", c); }
  }
} catch(e) { console.log("MAP VIEW THREW:", e.message); }

console.log("\n=== combined global+room buffer through room-view client ===");
const globalBuf = engineLine(cases.room_circle) + engineLine(cases.room_text_nan);
const roomBuf = engineLine(cases.room_poly);
const combined = "" + globalBuf + roomBuf; // backend rooms.js concat
try {
  console.log("drawn shapes:", roomViewParse(combined).join(','));
} catch(e) { console.log("ROOM VIEW THREW:", e.message); }
