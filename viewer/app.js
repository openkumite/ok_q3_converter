import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { Q3Shaders } from './q3shader.js';

THREE.ColorManagement.enabled = false; // Quake blends in gamma space; keep values untouched.

const $ = s => document.querySelector(s);
const el = (tag, attrs = {}, ...kids) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') e.className = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null && v !== false) e.setAttribute(k, v === true ? '' : v);
  }
  for (const k of kids.flat()) if (k != null) e.append(k.nodeType ? k : document.createTextNode(k));
  return e;
};
const base = p => p.split('/').pop().replace(/\.[^.]+$/, '');
const dir = p => p.split('/').slice(0, -1).join('/');
const fmtSize = n => n > 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n > 1e3 ? `${(n / 1e3).toFixed(0)} KB` : `${n} B`;

const [manifest, textureIndex, shaders] = await Promise.all(
  ['manifest.json', 'textures.json', 'shaders.json'].map(u => fetch(u).then(r => r.ok ? r.json() : {})));
const videoIndex = Object.fromEntries((manifest.videos || []).map(v => [v.path.replace(/\.roq$/, ''), v.url]));
const q3 = new Q3Shaders(shaders, textureIndex, videoIndex);
const sounds = manifest.sounds || [];
const canOgg = !!new Audio().canPlayType('audio/ogg; codecs="vorbis"');
const soundUrl = s => (canOgg && s.ogg) ? s.ogg : s.wav;

// ---------------------------------------------------------------- 3D stage
const canvas = $('#canvas');
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
renderer.outputColorSpace = THREE.LinearSRGBColorSpace;
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0d0c0b);
const camera = new THREE.PerspectiveCamera(70, 1, 0.02, 2000);
scene.add(new THREE.HemisphereLight(0xffffff, 0x404040, 1.6));
const sun = new THREE.DirectionalLight(0xffffff, 1.4);
sun.position.set(3, 6, 4);
scene.add(sun);
const orbit = new OrbitControls(camera, canvas);
window.__q3 = { camera, orbit, scene };
orbit.enableDamping = true;
const loader = new GLTFLoader();
const clock = new THREE.Clock();
let content = null;      // current root object
let mixer = null;
let q3meshes = [];
let fly = null;
let shaderTime = 0;
let onFrame = null;

function resize() {
  const r = canvas.parentElement.getBoundingClientRect();
  if (!r.width) return;
  renderer.setSize(r.width, r.height, false);
  camera.aspect = r.width / r.height;
  camera.updateProjectionMatrix();
}
new ResizeObserver(resize).observe($('#view3d'));

function clearStage() {
  if (content) {
    scene.remove(content);
    content.traverse(o => {
      if (o.geometry && !o.userData.q3pass) o.geometry.dispose();
      const mats = Array.isArray(o.material) ? o.material : [o.material];
      for (const m of mats) m?.dispose?.();
    });
  }
  content = null; mixer = null; q3meshes = []; onFrame = null;
  q3.live.clear();
  setFly(false);
  $('#toolbar').replaceChildren();
  $('#hud').textContent = '';
}

let loadToken = 0;
async function loadGlb(url, kind) {
  const token = loadToken;
  $('#loading').hidden = false;
  try {
    const gltf = await loader.loadAsync(url);
    if (token !== loadToken) throw new Error('stale');
    const root = gltf.scene;
    root.animations = gltf.animations;
    root.traverse(o => { if (o.isMesh && o.morphTargetInfluences) o.frustumCulled = false; });
    q3meshes = q3.apply(root, kind);
    content = root;
    scene.add(root);
    return root;
  } finally {
    $('#loading').hidden = true;
  }
}

function frameObject(obj, dist = 2.2) {
  const box = new THREE.Box3().setFromObject(obj, true);
  const size = box.getSize(new THREE.Vector3()).length() || 1;
  const c = box.getCenter(new THREE.Vector3());
  orbit.target.copy(c);
  camera.position.copy(c).add(new THREE.Vector3(0.6, 0.35, 1).normalize().multiplyScalar(size * dist * 0.5));
  camera.near = size / 500; camera.far = size * 100;
  camera.updateProjectionMatrix();
  orbit.update();
}

// Fly camera for maps: drag to look, WASD/QE/Space/C to move, Shift for speed.
const keys = new Set();
addEventListener('keydown', e => { if (e.target.tagName !== 'INPUT') keys.add(e.code); });
addEventListener('keyup', e => keys.delete(e.code));
addEventListener('blur', () => keys.clear());
function setFly(on, pos, yaw = 0) {
  orbit.enabled = !on;
  if (!on) { fly = null; return; }
  fly = { yaw, pitch: 0, speed: 8 };
  if (pos) camera.position.copy(pos);
  camera.near = 0.03; camera.far = 1500; camera.updateProjectionMatrix();
}
let drag = null;
canvas.addEventListener('pointerdown', e => { if (fly) { drag = { x: e.clientX, y: e.clientY }; canvas.setPointerCapture(e.pointerId); } });
canvas.addEventListener('pointermove', e => {
  if (!fly || !drag) return;
  fly.yaw -= (e.clientX - drag.x) * 0.004;
  fly.pitch = Math.max(-1.55, Math.min(1.55, fly.pitch - (e.clientY - drag.y) * 0.004));
  drag = { x: e.clientX, y: e.clientY };
});
canvas.addEventListener('pointerup', () => (drag = null));
canvas.addEventListener('wheel', e => { if (fly) { fly.speed = Math.max(1, Math.min(60, fly.speed * (e.deltaY < 0 ? 1.15 : 0.87))); e.preventDefault(); } }, { passive: false });

function stepFly(dt) {
  camera.rotation.set(fly.pitch, fly.yaw, 0, 'YXZ');
  const f = new THREE.Vector3(0, 0, -1).applyEuler(camera.rotation);
  const r = new THREE.Vector3(1, 0, 0).applyEuler(camera.rotation);
  const v = new THREE.Vector3();
  if (keys.has('KeyW') || keys.has('ArrowUp')) v.add(f);
  if (keys.has('KeyS') || keys.has('ArrowDown')) v.sub(f);
  if (keys.has('KeyD') || keys.has('ArrowRight')) v.add(r);
  if (keys.has('KeyA') || keys.has('ArrowLeft')) v.sub(r);
  if (keys.has('Space') || keys.has('KeyE')) v.y += 1;
  if (keys.has('KeyC') || keys.has('KeyQ')) v.y -= 1;
  if (v.lengthSq()) camera.position.addScaledVector(v.normalize(), fly.speed * dt * (keys.has('ShiftLeft') || keys.has('ShiftRight') ? 3 : 1));
}

function loop() {
  requestAnimationFrame(loop);
  if ($('#view3d').hidden) return;
  const dt = Math.min(clock.getDelta(), 0.1);
  shaderTime += dt;
  q3.update(shaderTime);
  mixer?.update(dt);
  if (fly) stepFly(dt); else orbit.update();
  onFrame?.(dt);
  renderer.render(scene, camera);
}
loop();

// ---------------------------------------------------------------- toolbar helpers
const tb = (...kids) => { const d = el('div', { class: 'tb' }, ...kids); $('#toolbar').append(d); return d; };
function tbSelect(label, options, value, onchange) {
  const s = el('select', { onchange: () => onchange(s.value) },
    options.map(o => el('option', { value: o.value ?? o, selected: (o.value ?? o) === value }, o.label ?? o)));
  tb(el('span', { class: 'muted' }, label), s);
  return s;
}
function tbCheck(label, checked, onchange) {
  const c = el('input', { type: 'checkbox', checked, onchange: () => onchange(c.checked) });
  tb(el('label', {}, c, ' ', label));
  return c;
}
function tbButton(label, onclick) {
  const d = el('div', { class: 'tb btn', onclick }, label);
  $('#toolbar').append(d);
  return d;
}
function shaderToggle() {
  tbCheck('Shader FX', true, on => q3.setEnabled(q3meshes, on));
}

// ---------------------------------------------------------------- audio
const player = new Audio();
let playingBtn = null;
function playSound(s, btn) {
  if (playingBtn === btn && !player.paused) { player.pause(); btn?.classList.remove('on'); playingBtn = null; return; }
  playingBtn?.classList.remove('on');
  player.src = soundUrl(s);
  player.play();
  playingBtn = btn;
  btn?.classList.add('on');
}
player.addEventListener('ended', () => { playingBtn?.classList.remove('on'); playingBtn = null; });
function soundRows(list) {
  return el('div', { class: 'rows' }, list.map(s => {
    const b = el('button', { class: 'play', title: 'Play', onclick: () => playSound(s, b) }, '▶');
    return el('div', { class: 'row' }, b,
      el('div', {}, base(s.path), el('div', { class: 'muted', style: 'font-size:11px' }, dir(s.path))),
      el('span', { class: 'muted' }, s.duration ? `${s.duration.toFixed(2)}s` : ''),
      el('span', {}, s.ogg ? el('a', { href: s.ogg, download: true }, 'ogg') : null, ' ', el('a', { href: s.wav, download: true }, 'wav')));
  }));
}

// ---------------------------------------------------------------- tabs
const WEAPON_SOUNDS = { rocketl: 'rocket', grenadel: 'grenade', plasma: 'plasma', machinegun: 'machinegun', lightning: 'lightning',
  railgun: 'railgun', shotgun: 'shotgun', bfg: 'bfg', gauntlet: 'melee', grapple: 'grapple' };
const isAnimatedShader = n => {
  const s = shaders[n];
  return s && (s.params.some(p => p[0] === 'deformvertexes' || p[0] === 'skyparms') ||
    s.stages.some(st => st.some(d => ['tcmod', 'animmap', 'videomap'].includes(d[0]) || (d[0] === 'rgbgen' && d[1] === 'wave') || (d[0] === 'tcgen' && d[1] === 'environment'))));
};

const TABS = [
  {
    id: 'maps', label: 'Maps', view: '3d', items: () => manifest.maps || [],
    key: m => m.name, text: m => `${m.name} ${m.title || ''}`,
    row: m => [m.levelshot ? el('img', { src: m.levelshot, loading: 'lazy' }) : el('img'), m.name, m.title || ''],
    open: openMap,
  },
  {
    id: 'players', label: 'Players', view: '3d', items: () => manifest.players || [],
    key: p => `${p.name}_${p.skin}`, text: p => `${p.name} ${p.skin}`, group: p => p.name,
    row: p => [p.icon ? el('img', { src: p.icon, class: 'icon', loading: 'lazy' }) : el('img', { class: 'icon' }), p.skin, p.name],
    open: openPlayer, side: true,
  },
  {
    id: 'weapons', label: 'Weapons', view: '3d', items: () => manifest.weapons || [],
    key: w => w.name, text: w => w.name,
    row: w => [w.icon ? el('img', { src: w.icon, class: 'icon' }) : el('img', { class: 'icon' }), w.name, w.parts.join(' + ')],
    open: openWeapon, side: true,
  },
  {
    id: 'models', label: 'Models', view: '3d', items: () => (manifest.models || []).filter(m => !m.lod),
    key: m => m.path, text: m => m.path, sub: m => m.category, group: m => dir(m.path).replace('models/', ''),
    row: m => [null, base(m.path), `${m.frames} frame${m.frames > 1 ? 's' : ''} · ${m.surfaces} surf`],
    open: openModel, side: true,
  },
  {
    id: 'vfx', label: 'Shaders / VFX', view: '3d', items: () => Object.keys(shaders).sort().map(n => ({ name: n })),
    key: s => s.name, text: s => s.name, sub: s => shaders[s.name].file.replace('scripts/', ''),
    subExtra: [{ value: '@animated', label: 'Animated only' }],
    subTest: (s, v) => v === '@animated' ? isAnimatedShader(s.name) : shaders[s.name].file.replace('scripts/', '') === v,
    row: s => [null, s.name.split('/').pop(), dir(s.name)],
    open: openShader, side: true,
  },
  {
    id: 'textures', label: 'Textures', view: 'grid', items: () => manifest.textures || [],
    text: t => t.path, sub: t => t.path.split('/').slice(0, 2).join('/'),
  },
  {
    id: 'ui', label: 'UI / HUD', view: 'grid', items: () => manifest.ui || [],
    text: t => t.path, sub: t => t.path.split('/').slice(0, dir(t.path).split('/').length > 1 ? 2 : 1).join('/'),
  },
  {
    id: 'sounds', label: 'Sounds', view: 'rows', items: () => sounds,
    text: s => s.path, sub: s => s.path.split('/').slice(0, 3).join('/'),
  },
  { id: 'music', label: 'Music', view: 'music', items: () => manifest.music || [], text: s => s.path },
  { id: 'video', label: 'Video', view: 'video', items: () => manifest.videos || [], text: s => s.path },
  {
    id: 'files', label: 'Files', view: 'files', items: () => manifest.misc || [],
    text: f => f.path, sub: f => f.path.includes('/') ? f.path.split('/')[0] : '(root)',
  },
];

let tab = null;
let selected = null;
const nav = $('#tabs');
for (const t of TABS) {
  const n = t.items().length;
  t.btn = el('button', { onclick: () => showTab(t.id) }, t.label, el('small', {}, n));
  nav.append(t.btn);
}
$('#stats').textContent = `${(manifest.mods || []).join(' + ')} · ${(manifest.paks || []).length} paks`;
$('#search').addEventListener('input', () => renderList());
$('#subfilter').addEventListener('change', () => renderList());

function showTab(id, keepHash) {
  tab = TABS.find(t => t.id === id) || TABS[0];
  for (const t of TABS) t.btn.classList.toggle('on', t === tab);
  if (!keepHash) history.replaceState(null, '', `#${tab.id}`);
  $('#search').value = '';
  const sub = $('#subfilter');
  const items = tab.items();
  if (tab.sub) {
    const vals = [...new Set(items.map(tab.sub))].filter(Boolean).sort();
    sub.replaceChildren(el('option', { value: '' }, 'All'), ...(tab.subExtra || []).map(o => el('option', { value: o.value }, o.label)),
      ...vals.map(v => el('option', { value: v }, v)));
    sub.hidden = false;
  } else sub.hidden = true;
  const is3d = tab.view === '3d';
  $('#view3d').hidden = !is3d;
  $('#panel').className = tab.side ? 'side-panel' : '';
  $('#panel').replaceChildren();
  $('#side').classList.toggle('hidden', ['music', 'video'].includes(tab.view));
  clearStage();
  selected = null;
  if (is3d) resize();
  renderList();
}

function filtered() {
  const q = $('#search').value.toLowerCase().trim();
  const sv = $('#subfilter').value;
  return tab.items().filter(it => (!q || tab.text(it).toLowerCase().includes(q)) &&
    (!sv || (tab.subTest ? tab.subTest(it, sv) : tab.sub(it) === sv)));
}

function renderList() {
  const items = filtered();
  const list = $('#list');
  list.replaceChildren();
  if (tab.view === '3d') {
    let lastGroup = null;
    const frag = document.createDocumentFragment();
    for (const it of items.slice(0, 3000)) {
      if (tab.group) {
        const g = tab.group(it);
        if (g !== lastGroup) { frag.append(el('div', { class: 'group' }, g)); lastGroup = g; }
      }
      const [img, title, subtitle] = tab.row(it);
      const row = el('div', { class: 'item' + (selected === tab.key(it) ? ' on' : ''), onclick: () => select(it, row) },
        img, el('div', { class: 't' }, el('span', {}, title), el('small', {}, subtitle)));
      frag.append(row);
    }
    list.append(frag);
    if (!selected && items.length) select(items[0], list.querySelector('.item'));
  } else {
    list.append(el('div', { class: 'empty' }, `${items.length} items`));
    renderPanel(items);
  }
}

async function select(it, row) {
  selected = tab.key(it);
  loadToken++;
  for (const r of $('#list').querySelectorAll('.item.on')) r.classList.remove('on');
  row?.classList.add('on');
  const t = tab;
  clearStage();
  $('#panel').replaceChildren();
  try {
    await t.open(it);
  } catch (e) {
    if (e.message === 'stale') return;
    console.error(e);
    $('#hud').textContent = `Failed: ${e.message}`;
  }
}

// ---------------------------------------------------------------- 3D openers
let musicAudio = null;
async function openMap(m) {
  const root = await loadGlb(m.glb, 'bsp');
  scene.background = new THREE.Color(0x000000);
  mixer = new THREE.AnimationMixer(root);
  for (const c of root.animations) mixer.clipAction(c).play();
  const sp = m.spawn ? new THREE.Vector3(...m.spawn.origin) : new THREE.Box3().setFromObject(root).getCenter(new THREE.Vector3());
  setFly(true, sp, ((m.spawn?.yaw || 0) - 90) * Math.PI / 180);

  shaderToggle();
  const ents = root.getObjectByName('entities');
  // Entity markers for non-item point entities.
  const markers = new THREE.Group();
  const geo = new THREE.OctahedronGeometry(0.12);
  ents?.children.forEach(e => {
    if (e.children.length || !e.userData.classname) return;
    const cls = e.userData.classname;
    const color = cls.startsWith('info_player') ? 0x4caf50 : cls.startsWith('light') ? 0xffd54f : cls.includes('target') || cls.includes('trigger') ? 0x42a5f5 : 0xbdbdbd;
    const mk = new THREE.Mesh(geo, new THREE.MeshBasicMaterial({ color, wireframe: true }));
    e.getWorldPosition(mk.position);
    mk.userData.cls = cls;
    markers.add(mk);
  });
  markers.visible = false;
  root.add(markers);
  tbCheck('Entity markers', false, on => (markers.visible = on));
  tbCheck('Items', true, on => ents?.children.forEach(e => { if (e.children.length && !e.isMesh) e.visible = on; }));
  const track = m.music && (manifest.music || []).find(t => t.path.replace(/\.wav$/, '') === m.music.split(' ')[0].replace(/\.wav$/, ''));
  if (track) {
    tbCheck('Music', false, on => {
      musicAudio?.pause();
      if (on) { musicAudio = new Audio(soundUrl(track)); musicAudio.loop = true; musicAudio.play(); }
    });
  }
  tbButton('⟲ Spawn', () => setFly(true, sp, ((m.spawn?.yaw || 0) - 90) * Math.PI / 180));
  const info = renderer.info;
  onFrame = () => {
    $('#hud').textContent = `${m.name}${m.title ? ' — ' + m.title : ''}\nDrag: look · WASD: move · Space/C: up/down · Shift: fast · Wheel: speed (${fly?.speed.toFixed(0)})\n${info.render.calls} draws · ${(info.render.triangles / 1000).toFixed(0)}k tris`;
  };
}

function soundPanel(title, list) {
  if (!list.length) return [];
  return [el('h3', {}, `${title} (${list.length})`), soundRows(list)];
}

async function openPlayer(p) {
  const root = await loadGlb(p.glb, 'model');
  scene.background = new THREE.Color(0x24201c);
  frameObject(root, 2.4);
  mixer = new THREE.AnimationMixer(root);
  const clips = Object.fromEntries(root.animations.map(c => [c.name, c]));
  const names = root.animations.map(c => c.name);
  const legsNames = names.filter(n => n.startsWith('LEGS'));
  const torsoNames = names.filter(n => n.startsWith('TORSO'));
  const bothNames = names.filter(n => n.startsWith('BOTH'));
  const actions = {};
  const play = (part, name) => {
    actions[part]?.stop();
    if (part === 'both') { actions.legs?.stop(); actions.torso?.stop(); }
    else if (actions.both) { actions.both.stop(); actions.both = null; }
    if (!name || !clips[name]) return;
    const a = mixer.clipAction(clips[name]);
    const loops = clips[name].userData?.loop ?? /IDLE|RUN|WALK|BACK|SWIM|TURN|STAND/.test(name);
    a.setLoop(loops ? THREE.LoopRepeat : THREE.LoopOnce);
    a.clampWhenFinished = true;
    a.reset().play();
    actions[part] = a;
  };
  let legs = 'LEGS_IDLE', torso = 'TORSO_STAND';
  play('legs', legs); play('torso', torso);
  tbSelect('Legs', legsNames, legs, v => { legs = v; play('legs', v); if (!actions.torso) play('torso', torso); });
  tbSelect('Torso', torsoNames, torso, v => { torso = v; play('torso', v); if (!actions.legs) play('legs', legs); });
  tbSelect('Both', ['—', ...bothNames], '—', v => { if (v === '—') { play('legs', legs); play('torso', torso); } else play('both', v); });
  let weaponObj = null;
  tbSelect('Weapon', ['none', ...(manifest.weapons || []).map(w => w.name)], 'none', async v => {
    weaponObj?.removeFromParent(); weaponObj = null;
    const w = (manifest.weapons || []).find(x => x.name === v);
    if (!w) return;
    const g = await loader.loadAsync(w.glb);
    weaponObj = g.scene;
    const extra = q3.apply(weaponObj, 'model');
    q3meshes.push(...extra);
    weaponObj.getObjectByName('flash')?.traverse(o => (o.visible = false));
    root.getObjectByName('tag_weapon')?.add(weaponObj);
  });
  tbCheck('Pause', false, on => (mixer.timeScale = on ? 0 : 1));
  shaderToggle();
  $('#hud').textContent = `${p.name} / ${p.skin}\nDrag: orbit · Wheel: zoom · Right-drag: pan`;
  const snd = sounds.filter(s => s.path.startsWith(p.sounds));
  $('#panel').replaceChildren(el('h2', {}, `${p.name} — ${p.skin}`),
    el('div', { class: 'kv' }, el('span', {}, 'File'), el('span', {}, el('a', { href: p.glb, download: true }, p.glb)),
      el('span', {}, 'Clips'), el('span', {}, `${names.length}`)),
    el('h3', {}, 'Animation clips'),
    el('div', { class: 'chips' }, names.map(n => el('span', { class: 'chip', onclick: () => play(n.startsWith('BOTH') ? 'both' : n.startsWith('LEGS') ? 'legs' : 'torso', n) }, n))),
    ...soundPanel('Voice & sounds', snd));
}

async function openWeapon(w) {
  const root = await loadGlb(w.glb, 'model');
  scene.background = new THREE.Color(0x24201c);
  const flash = root.getObjectByName('flash');
  const flashParent = flash?.parent;
  flash?.removeFromParent();
  frameObject(root, 1.6);
  flashParent?.add(flash);
  mixer = new THREE.AnimationMixer(root);
  const spin = root.animations.find(a => a.name === 'barrel_spin');
  if (flash) { flash.visible = false; tbCheck('Muzzle flash', false, on => (flash.visible = on)); }
  if (spin) { const a = mixer.clipAction(spin); tbCheck('Spin barrel', false, on => on ? a.play() : a.stop()); }
  let turn = true;
  tbCheck('Turntable', true, on => (turn = on));
  onFrame = dt => { if (turn) root.rotation.y += dt * 0.6; };
  shaderToggle();
  $('#hud').textContent = `${w.name}\nDrag: orbit · Wheel: zoom`;
  const key = WEAPON_SOUNDS[w.name] || w.name;
  const snd = sounds.filter(s => s.path.startsWith(`sound/weapons/${key}/`));
  $('#panel').replaceChildren(el('h2', {}, w.name),
    el('div', { class: 'kv' }, el('span', {}, 'File'), el('span', {}, el('a', { href: w.glb, download: true }, w.glb)),
      el('span', {}, 'Parts'), el('span', {}, w.parts.join(', ') || '—')),
    ...soundPanel('Sounds', snd));
}

async function openModel(m) {
  const root = await loadGlb(m.glb, 'model');
  scene.background = new THREE.Color(0x24201c);
  frameObject(root, 1.8);
  mixer = new THREE.AnimationMixer(root);
  if (root.animations.length) {
    const a = mixer.clipAction(root.animations[0]); a.play();
    tbCheck('Animate', true, on => (a.paused = !on));
  }
  let turn = m.category === 'items';
  tbCheck('Turntable', turn, on => (turn = on));
  onFrame = dt => { if (turn) root.rotation.y += dt * Math.PI * 2 / 2.048; };
  shaderToggle();
  $('#hud').textContent = `${m.path}\nDrag: orbit · Wheel: zoom`;
  const shaderNames = new Set();
  root.traverse(o => { if (o.userData.gltfMaterial) shaderNames.add(o.userData.gltfMaterial.userData.q3shader); });
  $('#panel').replaceChildren(el('h2', {}, base(m.path)),
    el('div', { class: 'kv' }, el('span', {}, 'Source'), el('span', {}, m.path),
      el('span', {}, 'File'), el('span', {}, el('a', { href: m.glb, download: true }, m.glb)),
      el('span', {}, 'Frames'), el('span', {}, m.frames), el('span', {}, 'Tags'), el('span', {}, m.tags.join(', ') || '—')),
    el('h3', {}, 'Shaders'), ...[...shaderNames].map(n => el('pre', { class: 'src' }, q3.source(n))));
}

async function openShader(s) {
  const sh = shaders[s.name];
  const isSky = sh.params.some(p => p[0] === 'skyparms');
  let geo;
  let shape = isSky ? 'sky' : 'sphere';
  const make = () => {
    clearStage();
    const g = shape === 'cube' ? new THREE.BoxGeometry(1, 1, 1) : shape === 'plane' ? new THREE.PlaneGeometry(1.4, 1.4)
      : shape === 'sky' ? new THREE.SphereGeometry(40, 48, 24) : new THREE.SphereGeometry(0.6, 64, 32);
    g.setAttribute('q3lm', g.attributes.uv);
    g.setAttribute('q3color', new THREE.Float32BufferAttribute(new Float32Array(g.attributes.position.count * 4).fill(1), 4));
    const mats = q3.build(s.name, { kind: 'model', lightmap: false });
    const grp = new THREE.Group();
    mats.forEach((m, i) => {
      if (shape === 'sky' || shape === 'plane') m.side = THREE.DoubleSide;
      const mesh = new THREE.Mesh(g, m); mesh.renderOrder = i; grp.add(mesh);
    });
    content = grp; scene.add(grp);
    scene.background = new THREE.Color(0x1a1714);
    if (shape === 'sky') { orbit.target.set(0, 0, 0); camera.position.set(0.01, 0, 0.01); camera.near = 0.01; camera.far = 200; camera.updateProjectionMatrix(); }
    else frameObject(grp, 2.2);
    tbSelect('Preview', ['sphere', 'cube', 'plane', 'sky'], shape, v => { shape = v; make(); });
    $('#hud').textContent = `${s.name}\n${mats.length} pass${mats.length > 1 ? 'es' : ''}`;
  };
  make();
  const imgs = new Set();
  for (const st of sh.stages) for (const d of st) {
    if ((d[0] === 'map' || d[0] === 'clampmap') && !d[1].startsWith('$')) imgs.add(d[1]);
    if (d[0] === 'animmap') d.slice(2).forEach(f => imgs.add(f));
  }
  const url = p => textureIndex[p.replace(/\.[a-z]+$/, '')];
  $('#panel').replaceChildren(el('h2', {}, s.name.split('/').pop()),
    el('pre', { class: 'src' }, q3.source(s.name)),
    imgs.size ? el('h3', {}, 'Images') : '',
    el('div', { class: 'grid' }, [...imgs].map(p => url(p) ? el('div', { class: 'tile', onclick: () => lightbox(url(p), p) },
      el('div', { class: 'img' }, el('img', { src: url(p), loading: 'lazy' })), el('div', { class: 'cap' }, base(p))) : null)));
}

// ---------------------------------------------------------------- non-3D panels
function lightbox(src, caption) {
  const lb = $('#lightbox');
  lb.replaceChildren(el('figure', {}, el('img', { src }), el('figcaption', {}, caption, ' · ', el('a', { href: src, download: true, onclick: e => e.stopPropagation() }, 'download'))));
  lb.hidden = false;
}
$('#lightbox').addEventListener('click', () => ($('#lightbox').hidden = true));
addEventListener('keydown', e => { if (e.key === 'Escape') $('#lightbox').hidden = true; });

function renderPanel(items) {
  const panel = $('#panel');
  panel.replaceChildren();
  if (tab.view === 'grid') {
    const LIMIT = 600;
    panel.append(el('div', { class: 'grid' }, items.slice(0, LIMIT).map(t =>
      el('div', { class: 'tile', onclick: () => lightbox(t.url, `${t.path} · ${t.w}×${t.h}${t.alpha ? ' · alpha' : ''}`) },
        el('div', { class: 'img' }, el('img', { src: t.url, loading: 'lazy' })),
        el('div', { class: 'cap' }, base(t.path), el('small', {}, `${t.w}×${t.h}`))))));
    if (items.length > LIMIT) panel.append(el('p', { class: 'muted' }, `Showing ${LIMIT} of ${items.length}. Use the filter to narrow down.`));
  } else if (tab.view === 'rows') {
    panel.append(soundRows(items.slice(0, 1500)));
  } else if (tab.view === 'music') {
    panel.append(el('div', { class: 'music' }, el('h2', {}, 'Soundtrack'), items.map(s => {
      const usedBy = (manifest.maps || []).filter(m => m.music && m.music.split(' ')[0].replace(/\.wav$/, '') === s.path.replace(/\.wav$/, '')).map(m => m.name);
      return el('div', { class: 'card' }, el('strong', {}, base(s.path)), ' ',
        el('span', { class: 'muted' }, `${(s.duration / 60 | 0)}:${String(Math.round(s.duration % 60)).padStart(2, '0')}`),
        usedBy.length ? el('div', { class: 'muted', style: 'font-size:11px;margin-top:3px' }, `Maps: ${usedBy.join(', ')}`) : '',
        el('audio', { controls: true, preload: 'none', src: soundUrl(s) }));
    })));
  } else if (tab.view === 'video') {
    panel.append(el('h2', {}, 'Cinematics'), items.length ? el('div', { class: 'vgrid' }, items.map(v =>
      el('div', {}, el('video', { controls: true, preload: 'metadata', src: v.url }), el('div', { class: 'muted' }, base(v.path)))))
      : el('p', { class: 'muted' }, 'No videos exported (run convert with ffmpeg available).'));
  } else if (tab.view === 'files') {
    const pre = el('pre', { class: 'src', hidden: true });
    panel.append(el('div', { class: 'rows' }, items.slice(0, 2000).map(f => {
      const text = /\.(c|h|cfg|txt|shader|skin|arena|bot|config|menu|dat)$/.test(f.path) || /\.(c|h)$/.test(f.path);
      return el('div', { class: 'row', style: 'grid-template-columns: 1fr auto auto' },
        el('span', { style: text ? 'cursor:pointer' : '', onclick: text ? async () => {
          pre.textContent = await fetch(f.url).then(r => r.text()); pre.hidden = false; pre.scrollIntoView({ behavior: 'smooth' });
        } : null }, f.path),
        el('span', { class: 'muted' }, fmtSize(f.size)),
        el('a', { href: f.url, download: true }, 'download'));
    })), pre);
  }
}

showTab((location.hash || '#maps').slice(1), true);
