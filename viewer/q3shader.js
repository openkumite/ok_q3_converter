// Quake 3 shader emulation for three.js.
// Each shader stage becomes one ShaderMaterial pass drawn over the same geometry,
// following the engine's rules for blendFunc, depth, tcGen/tcMod, rgbGen/alphaGen,
// animMap, deformVertexes and the sky/cloud projection.
import * as THREE from 'three';

const SCALE = 0.0254;

const BLEND = {
  gl_one: THREE.OneFactor, gl_zero: THREE.ZeroFactor,
  gl_dst_color: THREE.DstColorFactor, gl_one_minus_dst_color: THREE.OneMinusDstColorFactor,
  gl_src_alpha: THREE.SrcAlphaFactor, gl_one_minus_src_alpha: THREE.OneMinusSrcAlphaFactor,
  gl_src_color: THREE.SrcColorFactor, gl_one_minus_src_color: THREE.OneMinusSrcColorFactor,
  gl_dst_alpha: THREE.DstAlphaFactor, gl_one_minus_dst_alpha: THREE.OneMinusDstAlphaFactor,
  gl_src_alpha_saturate: THREE.SrcAlphaSaturateFactor,
};
const BLEND_SHORT = {
  add: ['gl_one', 'gl_one'], filter: ['gl_dst_color', 'gl_zero'],
  blend: ['gl_src_alpha', 'gl_one_minus_src_alpha'],
};
const SORT = { portal: 1, sky: 2, opaque: 3, decal: 4, seethrough: 5, banner: 6, additive: 10, nearest: 16, underwater: 8 };

function wave(func, base, amp, phase, freq, t) {
  let x = (phase + t * freq) % 1; if (x < 0) x += 1;
  let v;
  switch (func) {
    case 'sin': v = Math.sin(x * Math.PI * 2); break;
    case 'triangle': v = x < 0.5 ? 4 * x - 1 : 3 - 4 * x; break;
    case 'square': v = x < 0.5 ? 1 : -1; break;
    case 'sawtooth': v = x; break;
    case 'inversesawtooth': v = 1 - x; break;
    case 'noise': v = Math.sin(t * 13.37 + phase * 7.1) * 0.5 + Math.sin(t * 5.3) * 0.5; break;
    default: v = 0;
  }
  return base + amp * v;
}
const num = (s, d = 0) => { const v = parseFloat(s); return Number.isFinite(v) ? v : d; };
// Numbers in a directive, ignoring "(" ")" tokens.
const nums = (d, from) => d.slice(from).filter(t => t !== '(' && t !== ')').map(t => num(t.replace(/[()]/g, '')));
const WAVE_ID = { sin: 0, triangle: 1, square: 2, sawtooth: 3, inversesawtooth: 4, noise: 0 };

const VERT = /* glsl */`
#include <common>
#include <morphtarget_pars_vertex>
attribute vec2 q3lm;
attribute vec4 q3color;
uniform mat3 uTcPre, uTcPost;
uniform vec4 uTurb;        // amp, phase, freq, enabled
uniform int uTcGen;        // 0 base 1 lightmap 2 environment 3 vector 4 sky
uniform vec3 uTcVecS, uTcVecT;
uniform int uRgbGen;       // 0 uniform 1 vertex 2 oneMinusVertex 3 lightingDiffuse
uniform int uAlphaGen;     // 0 uniform 1 vertex 2 oneMinusVertex 3 lightingSpecular
uniform vec4 uDeform[4];   // per deform: (type, div/width, base/height, amp/speed)
uniform vec4 uDeformB[4];  // per deform: (func, phase, freq, 0)
uniform int uDeformCount;
uniform float uTime;
uniform float uCloudHeight;
varying vec2 vUv;
varying vec4 vColor;
varying vec3 vSkyDir;

float q3wave(float f, float base, float amp, float phase, float freq) {
  float x = fract(phase + uTime * freq);
  float v = sin(x * 6.2831853);
  if (f == 1.0) v = x < 0.5 ? 4.0 * x - 1.0 : 3.0 - 4.0 * x;
  else if (f == 2.0) v = x < 0.5 ? 1.0 : -1.0;
  else if (f == 3.0) v = x;
  else if (f == 4.0) v = 1.0 - x;
  return base + amp * v;
}

void main() {
  #include <beginnormal_vertex>
  #include <morphinstance_vertex>
  #include <morphnormal_vertex>
  #include <begin_vertex>
  #include <morphtarget_vertex>
  vec3 qpos = vec3(transformed.x, -transformed.z, transformed.y) / ${SCALE};
  for (int i = 0; i < 4; i++) {
    if (i >= uDeformCount) break;
    vec4 a = uDeform[i]; vec4 b = uDeformB[i];
    if (a.x == 1.0) {        // wave
      float off = (qpos.x + qpos.y + qpos.z) * a.y;
      transformed += objectNormal * q3wave(b.x, a.z, a.w, b.y + off, b.z) * ${SCALE};
    } else if (a.x == 2.0) { // bulge
      float off = uv.x * a.y + uTime * a.w;
      transformed += objectNormal * sin(off) * a.z * ${SCALE};
    }
  }
  vec4 world = modelMatrix * vec4(transformed, 1.0);
  vec3 n = normalize(mat3(modelMatrix) * objectNormal);
  vec3 toEye = normalize(cameraPosition - world.xyz);

  vec2 st = uv;
  if (uTcGen == 1) st = q3lm;
  else if (uTcGen == 2) {
    vec3 r = n * 2.0 * dot(n, toEye) - toEye;   // Q3 environment mapping (in Q3 axes)
    st = vec2(0.5 - r.z * 0.5, 0.5 - r.y * 0.5);
  } else if (uTcGen == 3) st = vec2(dot(qpos, uTcVecS), dot(qpos, uTcVecT));
  st = (uTcPre * vec3(st, 1.0)).xy;
  if (uTurb.w > 0.5) {
    float ph = uTurb.y + uTime * uTurb.z;
    st += vec2(sin(((qpos.x + qpos.z) * (1.0/128.0) * 0.125 + ph) * 6.2831853),
               sin((qpos.y * (1.0/128.0) * 0.125 + ph) * 6.2831853)) * uTurb.x;
  }
  vUv = (uTcPost * vec3(st, 1.0)).xy;

  vec3 d = world.xyz - cameraPosition;
  vSkyDir = vec3(d.x, -d.z, d.y);

  vec4 c = vec4(1.0);
  if (uRgbGen == 1) c.rgb = q3color.rgb;
  else if (uRgbGen == 2) c.rgb = 1.0 - q3color.rgb;
  else if (uRgbGen == 3) {
    vec3 L = normalize(vec3(0.4, 0.8, 0.45));
    c.rgb = vec3(clamp(0.45 + 0.75 * max(dot(n, L), 0.0), 0.0, 1.0));
  }
  if (uAlphaGen == 1) c.a = q3color.a;
  else if (uAlphaGen == 2) c.a = 1.0 - q3color.a;
  else if (uAlphaGen == 3) {
    vec3 L = normalize(vec3(-0.3, 0.9, 0.3));
    vec3 r = reflect(-L, n);
    c.a = pow(max(dot(r, toEye), 0.0), 4.0);
  }
  vColor = c;
  gl_Position = projectionMatrix * viewMatrix * world;
}`;

const FRAG = /* glsl */`
uniform sampler2D uMap;
uniform vec4 uColor;
uniform int uAlphaFunc;   // 0 none 1 GT0 2 LT128 3 GE128
uniform int uTcGen;
uniform int uSkyBox;      // 1: sample the six farbox faces
uniform sampler2D uSky[6];
uniform float uCloudHeight;
uniform mat3 uTcPre, uTcPost;
varying vec2 vUv;
varying vec4 vColor;
varying vec3 vSkyDir;

// The cloud dome: the view direction meets a sphere round the eye whose top is the cloud height.
vec2 cloudSt(vec3 v) {
  float R = 4096.0, h = uCloudHeight;
  float a = dot(v, v);
  float s = v.z*v.z*R*R + 2.0*(v.x*v.x + v.y*v.y + v.z*v.z)*R*h + (v.x*v.x + v.y*v.y + v.z*v.z)*h*h;
  float p = (1.0 / (2.0 * a)) * (-2.0 * v.z * R + 2.0 * sqrt(s));
  vec3 w = v * p; w.z += R;
  w = normalize(w);
  return vec2(acos(w.x), acos(w.y)) / 3.14159265;
}

vec4 skyBox(vec3 d) {
  vec3 a = abs(d);
  // Faces: 0 rt(+X) 1 bk(+Y) 2 lf(-X) 3 ft(-Y) 4 up(+Z) 5 dn(-Z), matching sky_texorder.
  vec2 uv; int f;
  if (a.x >= a.y && a.x >= a.z) {
    if (d.x > 0.0) { f = 0; uv = vec2(-d.y, d.z) / a.x; } else { f = 2; uv = vec2(d.y, d.z) / a.x; }
  } else if (a.y >= a.z) {
    if (d.y > 0.0) { f = 1; uv = vec2(d.x, d.z) / a.y; } else { f = 3; uv = vec2(-d.x, d.z) / a.y; }
  } else {
    if (d.z > 0.0) { f = 4; uv = vec2(-d.y, -d.x) / a.z; } else { f = 5; uv = vec2(-d.y, d.x) / a.z; }
  }
  uv = clamp(vec2(uv.x + 1.0, 1.0 - uv.y) * 0.5, 0.002, 0.998);
  if (f == 0) return texture2D(uSky[0], uv);
  if (f == 1) return texture2D(uSky[1], uv);
  if (f == 2) return texture2D(uSky[2], uv);
  if (f == 3) return texture2D(uSky[3], uv);
  if (f == 4) return texture2D(uSky[4], uv);
  return texture2D(uSky[5], uv);
}

void main() {
  vec4 t;
  if (uSkyBox == 1) t = skyBox(normalize(vSkyDir));
  else if (uTcGen == 4) {
    vec2 st = cloudSt(normalize(vSkyDir));
    st = (uTcPre * vec3(st, 1.0)).xy;
    st = (uTcPost * vec3(st, 1.0)).xy;
    t = texture2D(uMap, st);
  } else t = texture2D(uMap, vUv);
  vec4 c = t * vColor * uColor;
  if (uAlphaFunc == 1 && c.a <= 0.0) discard;
  if (uAlphaFunc == 2 && c.a >= 0.5) discard;
  if (uAlphaFunc == 3 && c.a < 0.5) discard;
  gl_FragColor = c;
}`;

function mat3Of(m) { const r = new THREE.Matrix3(); r.copy(m); return r; }

// Affine texture matrix for a single tcMod at time t (column-vector convention).
function tcModMatrix(d, t) {
  const m = new THREE.Matrix3();
  switch (d[0]) {
    case 'scroll': {
      let s = num(d[1]) * t, u = num(d[2]) * t;
      s -= Math.floor(s); u -= Math.floor(u);
      m.set(1, 0, s, 0, 1, u, 0, 0, 1); break;
    }
    case 'scale': m.set(num(d[1], 1), 0, 0, 0, num(d[2], 1), 0, 0, 0, 1); break;
    case 'rotate': {
      const a = -num(d[1]) * t * Math.PI / 180, c = Math.cos(a), s = Math.sin(a);
      m.set(c, -s, 0.5 - 0.5 * c + 0.5 * s, s, c, 0.5 - 0.5 * s - 0.5 * c, 0, 0, 1); break;
    }
    case 'stretch': {
      const w = wave(d[1], num(d[2]), num(d[3]), num(d[4]), num(d[5]), t);
      const p = w === 0 ? 1 : 1 / w;
      m.set(p, 0, 0.5 - 0.5 * p, 0, p, 0.5 - 0.5 * p, 0, 0, 1); break;
    }
    case 'transform': {
      const [m00, m01, m10, m11, t0, t1] = nums(d, 1);
      m.set(m00, m10, t0, m01, m11, t1, 0, 0, 1); break;
    }
  }
  return m;
}

export class Q3Shaders {
  constructor(shaders, textureIndex, videos = {}) {
    this.shaders = shaders;
    this.index = textureIndex;
    this.videos = videos;
    this.loader = new THREE.TextureLoader();
    this.cache = new Map();
    this.live = new Set();      // {material, stage} updated every frame
    this.white = this.solid(255, 255, 255);
    this.black = this.solid(0, 0, 0);
  }

  solid(r, g, b) {
    const t = new THREE.DataTexture(new Uint8Array([r, g, b, 255]), 1, 1);
    t.needsUpdate = true;
    return t;
  }

  texture(path, clamp = false) {
    if (!path) return this.white;
    if (path === '$whiteimage' || path === '*white') return this.white;
    const key = path.toLowerCase().replace(/\\/g, '/').replace(/\.[a-z]+$/, '');
    if (key.startsWith('video/')) return this.video(key);
    const url = this.index[key];
    if (!url) return this.white;
    const ck = url + (clamp ? '#c' : '');
    if (this.cache.has(ck)) return this.cache.get(ck);
    const tex = this.loader.load(url);
    tex.flipY = false;
    tex.colorSpace = THREE.NoColorSpace;
    tex.wrapS = tex.wrapT = clamp ? THREE.ClampToEdgeWrapping : THREE.RepeatWrapping;
    tex.anisotropy = 4;
    this.cache.set(ck, tex);
    return tex;
  }

  video(key) {
    if (this.cache.has(key)) return this.cache.get(key);
    const url = this.videos[key];
    if (!url) return this.black;
    const v = document.createElement('video');
    Object.assign(v, { src: url, loop: true, muted: true, playsInline: true, crossOrigin: 'anonymous' });
    v.play().catch(() => {});
    const t = new THREE.VideoTexture(v);
    t.flipY = false;
    this.cache.set(key, t);
    return t;
  }

  has(name) { return !!this.shaders[name]; }

  // Parsed, engine-style description of a shader (explicit or implicit).
  describe(name, ctx) {
    const sh = this.shaders[name];
    const out = { name, stages: [], sort: null, cull: 'front', deforms: [], polygonOffset: false, sky: null, fog: null, explicit: !!sh };
    if (!sh) {
      const tex = name;
      if (ctx.kind === 'bsp' && ctx.lightmap) {
        out.stages = [[['map', tex]], [['map', '$lightmap'], ['blendfunc', 'filter']]];
      } else if (ctx.kind === 'bsp') {
        out.stages = [[['map', tex], ['rgbgen', 'vertex']]];
      } else {
        out.stages = [[['map', tex], ['rgbgen', 'lightingdiffuse']]];
      }
      return out;
    }
    for (const p of sh.params) {
      switch (p[0]) {
        case 'cull': out.cull = p[1] || 'front'; break;
        case 'sort': out.sort = SORT[p[1]] ?? num(p[1], 3); break;
        case 'polygonoffset': out.polygonOffset = true; break;
        case 'deformvertexes': out.deforms.push(p); break;
        case 'skyparms': out.sky = { farbox: p[1], cloudheight: num(p[2], 128) || 128 }; break;
        case 'fogparms': out.fog = nums(p, 1); break;
      }
    }
    out.stages = sh.stages.filter(s => s.length);
    return out;
  }

  // Build the pass materials for a shader. ctx: {kind:'bsp'|'model', lightmap:bool, lmTex}
  build(name, ctx) {
    const d = this.describe(name, ctx);
    const mats = [];
    const side = { none: THREE.DoubleSide, disable: THREE.DoubleSide, twosided: THREE.DoubleSide,
                   back: THREE.BackSide, backside: THREE.BackSide, backsided: THREE.BackSide }[d.cull] ?? THREE.FrontSide;

    if (d.fog && !d.stages.length) {
      const [r, g, b] = d.fog;
      const m = new THREE.MeshBasicMaterial({ color: new THREE.Color(r, g, b), transparent: true, opacity: 0.35,
                                              depthWrite: false, side: THREE.DoubleSide });
      m.userData.sort = 9;
      return [m];
    }

    let sort = d.sort;
    if (d.sky) sort = SORT.sky;

    // Farbox pass for skies.
    if (d.sky && d.sky.farbox && this.index[`${d.sky.farbox}_rt`]) {
      const faces = ['rt', 'bk', 'lf', 'ft', 'up', 'dn'].map(s => this.texture(`${d.sky.farbox}_${s}`, true));
      const m = this.material({ uSkyBox: { value: 1 }, uSky: { value: faces } }, side);
      m.userData.sort = SORT.sky;
      mats.push(m);
    }

    d.stages.forEach((stage, i) => {
      const st = this.parseStage(stage, d, ctx);
      if (!st) return;
      const isFirst = mats.length === 0;
      const m = this.material({}, side);
      const u = m.uniforms;
      u.uMap.value = st.tex;
      u.uTcGen.value = d.sky ? 4 : st.tcGen;
      u.uCloudHeight.value = d.sky ? d.sky.cloudheight : 128;
      u.uRgbGen.value = st.rgbGenMode;
      u.uAlphaGen.value = st.alphaGenMode;
      u.uAlphaFunc.value = st.alphaFunc;
      if (st.tcVec) { u.uTcVecS.value.fromArray(st.tcVec[0]); u.uTcVecT.value.fromArray(st.tcVec[1]); }
      this.applyDeforms(u, d.deforms);
      if (st.blend) {
        m.blending = THREE.CustomBlending;
        m.blendSrc = st.blend[0];
        m.blendDst = st.blend[1];
        m.blendEquation = THREE.AddEquation;
        m.transparent = true;
      }
      m.depthWrite = st.depthWrite;
      m.depthFunc = st.depthEqual ? THREE.EqualDepth : THREE.LessEqualDepth;
      if (d.polygonOffset) Object.assign(m, { polygonOffset: true, polygonOffsetFactor: -1, polygonOffsetUnits: -2 });
      if (isFirst && sort == null) {
        sort = d.polygonOffset ? SORT.decal : st.blend ? (st.depthWrite ? SORT.seethrough : SORT.additive) : SORT.opaque;
      }
      m.userData.sort = sort ?? SORT.opaque;
      m.userData.stage = st;
      this.live.add(m);
      this.update1(m, 0);
      mats.push(m);
    });
    if (!mats.length) {
      const m = this.material({}, side);
      m.uniforms.uMap.value = this.texture(name);
      mats.push(m);
    }
    for (const m of mats) m.userData.sort ??= sort ?? SORT.opaque;
    return mats;
  }

  material(extra, side) {
    return new THREE.ShaderMaterial({
      vertexShader: VERT, fragmentShader: FRAG, side,
      uniforms: Object.assign({
        uMap: { value: this.white }, uColor: { value: new THREE.Vector4(1, 1, 1, 1) },
        uTcPre: { value: new THREE.Matrix3() }, uTcPost: { value: new THREE.Matrix3() },
        uTurb: { value: new THREE.Vector4() }, uTcGen: { value: 0 },
        uTcVecS: { value: new THREE.Vector3() }, uTcVecT: { value: new THREE.Vector3() },
        uRgbGen: { value: 0 }, uAlphaGen: { value: 0 }, uAlphaFunc: { value: 0 },
        uDeform: { value: [0, 1, 2, 3].map(() => new THREE.Vector4()) },
        uDeformB: { value: [0, 1, 2, 3].map(() => new THREE.Vector4()) },
        uDeformCount: { value: 0 }, uTime: { value: 0 }, uCloudHeight: { value: 128 },
        uSkyBox: { value: 0 }, uSky: { value: Array(6).fill(this.white) },
      }, extra),
    });
  }

  applyDeforms(u, deforms) {
    let n = 0;
    for (const p of deforms) {
      if (n >= 4) break;
      if (p[1] === 'wave') {
        const div = num(p[2]);
        u.uDeform.value[n].set(1, div ? 1 / div : 0, num(p[4]), num(p[5]));
        u.uDeformB.value[n].set(WAVE_ID[p[3]] ?? 0, num(p[6]), num(p[7]), 0);
        n++;
      } else if (p[1] === 'bulge') {
        u.uDeform.value[n].set(2, num(p[2]), num(p[3]), num(p[4]));
        n++;
      }
    }
    u.uDeformCount.value = n;
  }

  parseStage(stage, d, ctx) {
    const st = { tex: this.white, anim: null, tcGen: 0, tcMods: [], rgbGen: null, alphaGen: null,
                 blend: null, depthWrite: true, depthWriteExplicit: false, depthEqual: false, alphaFunc: 0,
                 rgbGenMode: 0, alphaGenMode: 0, isLightmap: false };
    let blendNames = null;
    for (const dir of stage) {
      switch (dir[0]) {
        case 'map':
          if (dir[1] === '$lightmap') {
            st.isLightmap = true; st.tcGen = 1;
            st.tex = ctx.lmTex || this.white;
          } else st.tex = this.texture(dir[1]);
          break;
        case 'clampmap': st.tex = this.texture(dir[1], true); break;
        case 'animmap': {
          st.anim = { freq: num(dir[1]), frames: dir.slice(2).map(f => this.texture(f)) };
          st.tex = st.anim.frames[0] || this.white;
          break;
        }
        case 'videomap': st.tex = this.texture(dir[1]); break;
        case 'blendfunc':
          blendNames = BLEND_SHORT[dir[1]] || [dir[1], dir[2]];
          break;
        case 'tcgen':
          if (dir[1] === 'environment') st.tcGen = 2;
          else if (dir[1] === 'lightmap') st.tcGen = 1;
          else if (dir[1] === 'vector') { const v = nums(dir, 2); st.tcGen = 3; st.tcVec = [v.slice(0, 3), v.slice(3, 6)]; }
          else st.tcGen = 0;
          break;
        case 'tcmod': st.tcMods.push(dir.slice(1)); break;
        case 'rgbgen': st.rgbGen = dir.slice(1); break;
        case 'alphagen': st.alphaGen = dir.slice(1); break;
        case 'alphafunc': st.alphaFunc = { gt0: 1, lt128: 2, ge128: 3 }[dir[1]] || 0; break;
        case 'depthwrite': st.depthWriteExplicit = true; break;
        case 'depthfunc': st.depthEqual = dir[1] === 'equal'; break;
      }
    }
    if (st.isLightmap && ctx.kind !== 'bsp') st.tex = this.white;
    if (blendNames) {
      const src = BLEND[blendNames[0]], dst = BLEND[blendNames[1]];
      if (src !== undefined && dst !== undefined && !(src === THREE.OneFactor && dst === THREE.ZeroFactor)) st.blend = [src, dst];
    }
    st.depthWrite = !st.blend || st.depthWriteExplicit;
    const rg = st.rgbGen?.[0];
    st.rgbGenMode = { vertex: 1, exactvertex: 1, oneminusvertex: 2, lightingdiffuse: 3 }[rg] || 0;
    // Implicit lightmapped surfaces whose shader has no lightmap stage (e.g. vertex lit maps): use vertex colors.
    const ag = st.alphaGen?.[0];
    st.alphaGenMode = { vertex: 1, onminusvertex: 2, oneminusvertex: 2, lightingspecular: 3 }[ag] || 0;
    // Split tcMods around the first turb so ordering is mostly preserved.
    const ti = st.tcMods.findIndex(m => m[0] === 'turb');
    st.pre = ti < 0 ? st.tcMods : st.tcMods.slice(0, ti);
    st.post = ti < 0 ? [] : st.tcMods.slice(ti + 1);
    st.turb = ti < 0 ? null : st.tcMods[ti];
    return st;
  }

  update1(m, t) {
    const st = m.userData.stage;
    if (!st) return;
    const u = m.uniforms;
    u.uTime.value = t;
    const compose = (mods, out) => {
      out.identity();
      for (const md of mods) out.premultiply(tcModMatrix(md, t));
    };
    compose(st.pre, u.uTcPre.value);
    compose(st.post, u.uTcPost.value);
    if (st.turb) {
      // turb: [turb, base?, amp, phase, freq] -- the engine ignores base.
      const v = nums(st.turb, 1);
      const [amp, phase, freq] = v.length >= 4 ? [v[1], v[2], v[3]] : [v[0], v[1], v[2]];
      u.uTurb.value.set(amp, phase, freq, 1);
    }
    if (st.anim && st.anim.frames.length) {
      const i = Math.floor(t * st.anim.freq) % st.anim.frames.length;
      u.uMap.value = st.anim.frames[i];
    }
    const c = u.uColor.value;
    c.set(1, 1, 1, 1);
    const rg = st.rgbGen;
    if (rg) {
      if (rg[0] === 'wave') { const w = Math.min(1, Math.max(0, wave(rg[1], num(rg[2]), num(rg[3]), num(rg[4]), num(rg[5]), t))); c.x = c.y = c.z = w; }
      else if (rg[0] === 'const' || rg[0] === 'constant') { const v = nums(rg, 1); c.x = v[0]; c.y = v[1]; c.z = v[2]; }
      else if (rg[0] === 'oneminusentity') { c.x = c.y = c.z = 0; }
    }
    const ag = st.alphaGen;
    if (ag) {
      if (ag[0] === 'wave') c.w = Math.min(1, Math.max(0, wave(ag[1], num(ag[2]), num(ag[3]), num(ag[4]), num(ag[5]), t)));
      else if (ag[0] === 'const' || ag[0] === 'constant') c.w = num(ag[1], 1);
      else if (ag[0] === 'oneminusentity') c.w = 0;
    }
  }

  update(t) { for (const m of this.live) this.update1(m, t); }

  // Swap glTF materials for shader passes on every mesh under `root`.
  apply(root, kind) {
    const meshes = [];
    root.traverse(o => { if (o.isMesh && o.material?.userData?.q3shader !== undefined) meshes.push(o); });
    for (const mesh of meshes) {
      const gm = mesh.material;
      const g = mesh.geometry;
      if (g.attributes.uv1 && !g.attributes.q3lm) g.setAttribute('q3lm', g.attributes.uv1);
      if (!g.attributes.q3color) {
        if (g.attributes.color) g.setAttribute('q3color', g.attributes.color);
        else {
          const n = g.attributes.position.count;
          g.setAttribute('q3color', new THREE.Float32BufferAttribute(new Float32Array(n * 4).fill(1), 4));
        }
      }
      const ctx = { kind, lightmap: !!gm.userData.lightmap, lmTex: gm.aoMap };
      if (gm.aoMap) { gm.aoMap.flipY = false; gm.aoMap.colorSpace = THREE.NoColorSpace; }
      const mats = this.build(gm.userData.q3shader, ctx);
      mesh.userData.gltfMaterial = gm;
      mesh.userData.q3passes = [];
      mats.forEach((m, i) => {
        let target = mesh;
        if (i > 0) {
          target = new THREE.Mesh(g, m);
          target.morphTargetInfluences = mesh.morphTargetInfluences;
          target.morphTargetDictionary = mesh.morphTargetDictionary;
          target.frustumCulled = mesh.frustumCulled;
          target.name = `${mesh.name}#pass${i}`;
          target.userData.q3pass = true;
          mesh.add(target);
        } else mesh.material = m;
        target.renderOrder = m.userData.sort * 100 + i;
        mesh.userData.q3passes.push(target);
      });
    }
    return meshes;
  }

  // Toggle between shader emulation and the plain glTF materials.
  setEnabled(meshes, on) {
    for (const mesh of meshes) {
      const passes = mesh.userData.q3passes || [];
      if (!mesh.userData.q3material) mesh.userData.q3material = mesh.material;
      mesh.material = on ? mesh.userData.q3material : mesh.userData.gltfMaterial;
      passes.slice(1).forEach(p => (p.visible = on));
    }
  }

  dispose() {
    this.live.clear();
  }

  // Readable shader source reconstructed from the parsed tokens.
  source(name) {
    const sh = this.shaders[name];
    if (!sh) return `// ${name}\n// (no script: implicit shader, texture * lightmap)`;
    const q = t => (/\s/.test(t) ? `"${t}"` : t);
    let s = `// ${sh.file}\n${name}\n{\n`;
    for (const p of sh.params) s += `\t${p.map(q).join(' ')}\n`;
    for (const st of sh.stages) {
      s += '\t{\n';
      for (const d of st) s += `\t\t${d.map(q).join(' ')}\n`;
      s += '\t}\n';
    }
    return s + '}\n';
  }
}
