-- CursorFX · gun-sniper
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：枪口即热点，枪身从鼠标处向后延伸。全部零件用局部多边形
-- 旋转/镜像后绘制，不依赖变换栈；六种皮肤用纯色分层、亮边、暗边与点缀线
-- 近似原版的线性渐变/裁剪纹理。按住左键会按射速连发，并持续产生后坐、
-- 枪口焰与抛壳。

name = "gun-sniper"
effect_name = "狙击枪"
kind = "cursor"
description = "CursorFX gun-sniper：霜冻长管狙击枪，按住连发，后坐与枪口焰更重。"
hide_cursor = true

local TAU = math.pi * 2

local SKIN_ORDER = { "gold", "neon", "crimson", "camo", "frost", "carbon" }
local SKIN_LABELS = { "黄金", "霓虹", "绯红", "迷彩", "霜冻", "碳纤维" }
local SKINS = {
  gold = {
    body = { 201, 144, 26 }, light = { 247, 207, 74 }, dark = { 111, 74, 8 },
    ink = { 74, 50, 4 }, accent = { 255, 243, 176 }, reticle = { 255, 243, 176 },
    shine = true, engrave = true,
  },
  neon = {
    body = { 23, 27, 40 }, light = { 42, 48, 68 }, dark = { 7, 8, 13 },
    ink = { 39, 243, 255 }, accent = { 255, 61, 242 }, reticle = { 39, 243, 255 },
    glow = { 39, 243, 255 },
  },
  crimson = {
    body = { 225, 29, 72 }, light = { 255, 138, 149 }, dark = { 76, 5, 25 },
    ink = { 42, 6, 16 }, accent = { 255, 255, 255 }, reticle = { 255, 255, 255 },
    shine = true, stripe = true,
  },
  camo = {
    body = { 91, 107, 58 }, light = { 138, 151, 92 }, dark = { 47, 58, 28 },
    ink = { 28, 35, 17 }, accent = { 216, 201, 154 }, reticle = { 216, 201, 154 },
    blotches = { { 61, 74, 38 }, { 138, 122, 74 }, { 43, 47, 30 } },
  },
  frost = {
    body = { 223, 244, 255 }, light = { 255, 255, 255 }, dark = { 79, 143, 196 },
    ink = { 29, 74, 115 }, accent = { 91, 209, 255 }, reticle = { 91, 209, 255 },
    shine = true, sparkle = true,
  },
  carbon = {
    body = { 44, 47, 54 }, light = { 90, 94, 104 }, dark = { 11, 12, 15 },
    ink = { 0, 0, 0 }, accent = { 255, 122, 26 }, reticle = { 255, 122, 26 },
    shine = true, weave = true,
  },
}

local function box(x, y, w, h)
  return { { x, y }, { x + w, y }, { x + w, y + h }, { x, y + h } }
end

local MODEL = {
  size = 0.78,
  default_skin = "frost",
  grip = { -102, 10 },
  port = { -92, -3 },
  minx = -141, maxx = 0, miny = -18, maxy = 17,
  fire_interval = 0.55,
  flash_scale = 1.35,
  parts = {
    box(-9, -3, 9, 6),
    box(-60, -1.6, 52, 3.2),
    { { -84, -3.6 }, { -58, -3.2 }, { -58, 3.2 }, { -84, 4.2 } },
    { { -104, -5 }, { -82, -5 }, { -82, 4 }, { -104, 4.5 } },
    box(-100, -14, 32, 5),
    { { -72, -15.6 }, { -62, -16.6 }, { -62, -6.4 }, { -72, -7.4 } },
    { { -106, -14.6 }, { -99, -14 }, { -99, -9 }, { -106, -8.4 } },
    box(-95, -9.2, 4, 4.4),
    box(-79, -9.2, 4, 4.4),
    box(-88, -17.4, 5, 3.6),
    { { -97, 4 }, { -88, 4 }, { -88, 11 }, { -97, 11 } },
    { { -106, 4 }, { -99, 4 }, { -101, 15 }, { -108, 16 } },
    { { -138, -5 }, { -104, -5 }, { -104, 4.5 }, { -116, 6 }, { -120, 12.5 }, { -138, 13.5 } },
    { { -134, -8 }, { -114, -7.2 }, { -114, -5 }, { -134, -5 } },
    box(-141, -5.5, 3, 19.5),
  },
  top_lines = {
    { -60, -1.6, -8, -1.6, 0.9 },
    { -84, -3.6, -58, -3.2, 0.9 },
    { -104, -5, -82, -5, 0.9 },
    { -100, -14, -68, -14, 0.9 },
    { -138, -5, -104, -5, 1.0 },
    { -134, -8, -114, -7.2, 0.8 },
  },
  bottom_lines = {
    { -60, 1.6, -8, 1.6, 0.9 },
    { -84, 4.2, -58, 3.2, 0.9 },
    { -104, 4.5, -82, 4, 0.9 },
    { -138, 13.5, -116, 6, 1.0 },
    { -108, 16, -101, 15, 0.9 },
  },
  ink_lines = {
    { -82, -5, -82, 4, 0.7, 130 },
    { -104, -5, -104, 4.5, 0.7, 130 },
    { -80, 1, -60, 1, 0.6, 130 },
    { -126, 0, -110, 0, 0.6, 130 },
    { -99, 5, -97, 10, 0.8, 220 },
    { -97, 10, -92, 9.4, 0.8, 220 },
    { -120, -3, -106, -3, 0.5, 90 },
  },
  ink_polys = {
    box(-96, -3.6, 9, 2.4),
    box(-89.6, 3.5, 1.2, 3),
  },
  ink_dots = { { -89, 6.5, 1.8 } },
  accent_polys = {
    box(-63.5, -15, 1.8, 8),
    { { -134, 3 }, { -122, 3.4 }, { -122, 5 }, { -134, 5.4 } },
  },
  accent_lines = {},
  loops = {},
  shine = { x1 = -138, x2 = -86, y1 = -4, y2 = 5 },
  stripe = { x1 = -130, x2 = -104, y = 0.5 },
  blots = { { -124, -1, 3.6 }, { -112, 2, 3.0 }, { -96, 3, 2.5 } },
  weave = {
    { -118, -4, -110, 4 }, { -114, -4, -106, 4 }, { -110, -4, -102, 4 },
  },
  sparkle = { { -128, -3 }, { -70, -11 } },
  engrave = {},
}

local gun = { kick = 0, sway = 0, cooldown = 0, casings = {} }

function schema()
  return {
    { key = "skin",    label = "皮肤",   type = "choice", choices = SKIN_ORDER, labels = SKIN_LABELS, default = "frost" },
    { key = "aim",     label = "朝向",   type = "choice", choices = { "left", "right" }, labels = { "向左", "向右" }, default = "left" },
    { key = "scale",   label = "大小",   type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
    { key = "recoil",  label = "后坐力", type = "float", min = 0.0, max = 2.0, step = 0.05, default = 1.0 },
    { key = "autoFire",label = "按住连发", type = "bool", default = true },
    { key = "reticle", label = "准星",   type = "bool", default = true },
    { key = "casings", label = "抛壳",   type = "bool", default = true },
  }
end

local function pose(params, kick)
  local mx = mouse_x or 0
  local my = mouse_y or 0
  local sw = screen_w or 1920
  local sh = screen_h or 1080
  local auto = clamp(math.min(sw, sh) / 800.0, 0.8, 1.2)
  local sc = (params.scale or 1.0) * 0.85 * MODEL.size * auto
  local aim_right = (params.aim or "left") == "right"
  local mirror = aim_right and 1.0 or -1.0
  local base = (aim_right and -0.42 or 0.42) + (gun.sway or 0)
  local cb, sb = math.cos(base), math.sin(base)
  local gx, gy = MODEL.grip[1], MODEL.grip[2]
  local recoil = params.recoil or 1.0
  local rr = -kick * 0.22 * recoil
  local cr, sr = math.cos(rr), math.sin(rr)

  local function w(lx, ly, expand)
    local e = expand or 1.0
    lx = gx + (lx - gx) * e
    ly = gy + (ly - gy) * e
    local dx, dy = lx - gx, ly - gy
    lx = gx + dx * cr - dy * sr - kick * 6.0 * recoil
    ly = gy + dx * sr + dy * cr
    local sx = lx * mirror * sc
    local sy = ly * sc
    return mx + sx * cb - sy * sb, my + sx * sb + sy * cb
  end

  return w, mx, my, sc
end

local function fire(params)
  params = params or {}
  gun.kick = 1.0
  if params.casings == false or #gun.casings >= 10 then return end
  local w = pose(params, 0.4)
  local px, py = w(MODEL.port[1], MODEL.port[2])
  local ox, oy = w(MODEL.port[1] - 5, MODEL.port[2] - 8)
  local dx, dy = ox - px, oy - py
  local d = math.sqrt(dx * dx + dy * dy)
  if d < 0.001 then dx, dy, d = 0, -1, 1 end
  local sp = rng(2.6, 4.2) * 60.0
  local life = rng(0.6, 0.95)
  gun.casings[#gun.casings + 1] = {
    x = px, y = py,
    vx = dx / d * sp + rng(-18, 18),
    vy = dy / d * sp + rng(-28, 4),
    rot = rng(0, TAU), spin = rng(-32, 32),
    life = life, max = life,
  }
end

function init()
  gun = { kick = 0, sway = 0, cooldown = 0, casings = {} }
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" then return end
  fire(params)
  gun.cooldown = MODEL.fire_interval
end

function step(dt, t, params)
  params = params or {}
  gun.kick = gun.kick * math.exp(-dt * 15.0)
  gun.cooldown = math.max(0, gun.cooldown - dt)
  local target = clamp((mouse_vx or 0) * 0.012, -0.25, 0.25)
  gun.sway = gun.sway + (target - gun.sway) * (1 - math.exp(-dt * 9.0))

  if mouse_down and mouse_button == "left" and params.autoFire ~= false and gun.cooldown <= 0 then
    fire(params)
    gun.cooldown = MODEL.fire_interval
  end

  for i = #gun.casings, 1, -1 do
    local c = gun.casings[i]
    c.life = c.life - dt
    if c.life <= 0 then
      table.remove(gun.casings, i)
    else
      c.vy = c.vy + 1400 * dt
      c.x = c.x + c.vx * dt
      c.y = c.y + c.vy * dt
      c.rot = c.rot + c.spin * dt
    end
  end
end

local function draw_poly(ctx, w, pts, col, a, expand)
  local out = {}
  for i = 1, #pts do
    local x, y = w(pts[i][1], pts[i][2], expand)
    out[i] = { x, y }
  end
  ctx:polygon(out, col[1], col[2], col[3], a)
end

local function fill_poly(ctx, w, pts, col, a)
  local out = {}
  for i = 1, #pts do
    local x, y = w(pts[i][1], pts[i][2])
    out[i] = { x, y }
  end
  ctx:fill_polygon(out, col[1], col[2], col[3], a)
end

local function draw_line(ctx, w, x1, y1, x2, y2, width, col, a)
  local ax, ay = w(x1, y1)
  local bx, by = w(x2, y2)
  ctx:line(ax, ay, bx, by, math.max(0.5, width), col[1], col[2], col[3], a)
end

local function draw_dot(ctx, w, x, y, radius, col, a)
  local px, py = w(x, y)
  ctx:circle(px, py, math.max(0.5, radius), col[1], col[2], col[3], a)
end

local function paint_extras(ctx, w, sc, pal)
  if pal.engrave then
    local lines = MODEL.engrave or {}
    for i = 1, #lines do
      local l = lines[i]
      draw_line(ctx, w, l[1], l[2], l[3], l[4], 0.6 * sc, pal.ink, 80)
    end
  end
  if pal.blotches then
    local blots = MODEL.blots or {}
    for i = 1, #blots do
      local b = blots[i]
      local col = pal.blotches[(i - 1) % #pal.blotches + 1]
      draw_dot(ctx, w, b[1], b[2], b[3] * sc, col, 215)
    end
  end
  if pal.weave then
    local lines = MODEL.weave or {}
    for i = 1, #lines do
      local l = lines[i]
      draw_line(ctx, w, l[1], l[2], l[3], l[4], 0.7 * sc, { 255, 255, 255 }, 38)
    end
  end
  if pal.sparkle then
    local pts = MODEL.sparkle or {}
    local pulse = 0.5 + 0.5 * math.sin((elapsed or 0) * 3.0)
    for i = 1, #pts do
      local p = pts[i]
      local px, py = w(p[1], p[2])
      ctx:plus_circle(px, py, (1.2 + pulse * 1.8) * sc, 255, 255, 255,
        math.floor(150 + pulse * 90))
    end
  end
  if pal.shine then
    local sh = MODEL.shine
    local t = ((elapsed or 0) * 0.55) % 1.0
    local sx = sh.x1 + t * (sh.x2 - sh.x1)
    draw_line(ctx, w, sx, sh.y1, sx + 4, sh.y2, 2.0 * sc, pal.light, 75)
  end
  if pal.stripe then
    local st = MODEL.stripe
    draw_line(ctx, w, st.x1, st.y, st.x2, st.y, 2.2 * sc, pal.accent, 215)
  end
end

local function muzzle_flash(ctx, w, sc, kick, pal)
  if kick <= 0.18 then return end
  local k = math.min(1.0, kick)
  local fx, fy = w(4, 0)
  local r = (4 + 14 * k) * sc * MODEL.flash_scale
  local a = math.floor(240 * math.min(1, k * 1.2))
  ctx:radial_stops(fx, fy, r, {
    { offset = 0.0, r = 255, g = 250, b = 220, a = a },
    { offset = 0.38, r = 255, g = 190, b = 70, a = math.floor(a * 0.62) },
    { offset = 1.0, r = 255, g = 80, b = 20, a = 0 },
  })
  fill_poly(ctx, w, { { 0, -2.2 }, { 22 * k + 6, 0 }, { 0, 2.2 } }, { 255, 230, 140 }, math.floor(210 * k))
  fill_poly(ctx, w, { { 0, -1 }, { 11 * k, -8 * k }, { 3, 0 } }, { 255, 210, 90 }, math.floor(170 * k))
  fill_poly(ctx, w, { { 0, 1 }, { 11 * k, 8 * k }, { 3, 0 } }, { 255, 210, 90 }, math.floor(170 * k))
  if MODEL.flash_ring then
    ctx:ring(fx, fy, r * 1.15, 2 * sc, pal.accent[1], pal.accent[2], pal.accent[3], math.floor(140 * k))
  end
end

local function draw_casings(ctx)
  for i = 1, #gun.casings do
    local c = gun.casings[i]
    local ca, sa = math.cos(c.rot), math.sin(c.rot)
    local a = math.floor(255 * math.min(1, c.life / 0.2))
    ctx:line(c.x - 3.5 * ca, c.y - 3.5 * sa, c.x + 3.3 * ca, c.y + 3.3 * sa, 2.4, 217, 169, 60, a)
    ctx:line(c.x - 3.5 * ca, c.y - 3.5 * sa, c.x - 2.2 * ca, c.y - 2.2 * sa, 2.4, 138, 106, 30, a)
  end
end

function draw(ctx, params)
  params = params or {}
  local pal = SKINS[params.skin] or SKINS[MODEL.default_skin]
  local kick = gun.kick or 0
  local held = mouse_down and mouse_button == "left"
  local w, mx, my, sc = pose(params, kick)

  if pal.glow then
    for i = 1, #MODEL.parts do
      draw_poly(ctx, w, MODEL.parts[i], pal.glow, 66, 1.055)
    end
  end

  for i = 1, #MODEL.parts do
    draw_poly(ctx, w, MODEL.parts[i], pal.body, 255)
  end

  for i = 1, #MODEL.top_lines do
    local l = MODEL.top_lines[i]
    draw_line(ctx, w, l[1], l[2], l[3], l[4], (l[5] or 1) * sc, pal.light, 210)
  end
  for i = 1, #MODEL.bottom_lines do
    local l = MODEL.bottom_lines[i]
    draw_line(ctx, w, l[1], l[2], l[3], l[4], (l[5] or 1) * sc, pal.dark, 220)
  end

  paint_extras(ctx, w, sc, pal)

  for i = 1, #MODEL.ink_polys do
    draw_poly(ctx, w, MODEL.ink_polys[i], pal.ink, 255)
  end
  for i = 1, #MODEL.ink_lines do
    local l = MODEL.ink_lines[i]
    draw_line(ctx, w, l[1], l[2], l[3], l[4], (l[5] or 0.8) * sc, pal.ink, l[6] or 255)
  end
  for i = 1, #MODEL.ink_dots do
    local d = MODEL.ink_dots[i]
    draw_dot(ctx, w, d[1], d[2], d[3] * sc, pal.ink, 255)
  end

  for i = 1, #MODEL.accent_polys do
    draw_poly(ctx, w, MODEL.accent_polys[i], pal.accent, 230)
  end
  for i = 1, #MODEL.accent_lines do
    local l = MODEL.accent_lines[i]
    draw_line(ctx, w, l[1], l[2], l[3], l[4], (l[5] or 1) * sc, pal.accent, l[6] or 230)
  end
  for i = 1, #MODEL.loops do
    local lp = MODEL.loops[i]
    local lx, ly = w(lp[1], lp[2])
    ctx:ring(lx, ly, lp[3] * sc, (lp[4] or 2) * sc, pal.ink[1], pal.ink[2], pal.ink[3], 255)
    ctx:ring(lx, ly, lp[3] * sc, (lp[4] or 2) * 0.55 * sc, pal.light[1], pal.light[2], pal.light[3], 220)
  end

  muzzle_flash(ctx, w, sc, kick, pal)
  draw_casings(ctx)

  if params.reticle ~= false then
    local r = 5 + kick * 4 + (held and 0.8 or 0)
    local t = 3
    ctx:line(mx - r - t, my, mx - r, my, 3.2, 0, 0, 0, 170)
    ctx:line(mx + r, my, mx + r + t, my, 3.2, 0, 0, 0, 170)
    ctx:line(mx, my - r - t, mx, my - r, 3.2, 0, 0, 0, 170)
    ctx:line(mx, my + r, mx, my + r + t, 3.2, 0, 0, 0, 170)
    ctx:circle(mx, my, 2.3, 0, 0, 0, 170)
    local cr, cg, cb = pal.reticle[1], pal.reticle[2], pal.reticle[3]
    ctx:line(mx - r - t, my, mx - r, my, 1.3, cr, cg, cb, 235)
    ctx:line(mx + r, my, mx + r + t, my, 1.3, cr, cg, cb, 235)
    ctx:line(mx, my - r - t, mx, my - r, 1.3, cr, cg, cb, 235)
    ctx:line(mx, my + r, mx, my + r + t, 1.3, cr, cg, cb, 235)
    ctx:circle(mx, my, 1.2, cr, cg, cb, 235)
  end
end
