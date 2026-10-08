import { $d as needsTransformation, $f as f32, $l as listHasDrivers, $u as frameGate, Bl as boxHalfExtentsUV, Bu as SHAPE3D_TYPES, Cl as createGpuUniformsMap, Dd as packConvertedStops, Gf as builtin, Gl as resolveDimensionalProp, Gu as isVolumetricShapeType, Hf as vec2f$1, Hl as buildExtraSizeProps, Hu as SVG3D_TYPES, Jl as setColorSpaceModeGpu, Ju as applySpring, Kl as setShaderCanvasDimensions, Ku as shape3dBoundingRadius, Lc as rootPassthrough, Md as createComputeDispatcher, Od as packStops, Qd as boundingBoxToUVParams, Qf as vec2f, Ql as listFieldName, Qu as deriveElapsedTime, Rd as SHARED_SAMPLER_LAYOUT, Sd as registerNaturalSize, Sf as vertexFn, Ud as collectStructuralHashInputs, Ul as isDimensionalProp, Uu as is3dShapeType, Vl as buildDimensionalPlan, Vu as SVG3D_DEFAULTS, Wd as composeNodeTree, Wl as isDimensionalValue, Wu as isSvg3dShapeType, Xd as isIdentityBoundingBox, Xf as struct, Xl as listCountName, Xu as clampToTextureCap, Yd as DEFAULT_BOUNDING_BOX, Yl as isPositionDriver, Yu as awaitGpuIdle, Zd as resolveBoundingBox, Zf as mul, Zl as listDriverPath, Zu as createFrameLoop, _f as ArrayFieldHandle, a as unregisterShader, au as MAX_COLOR_STOPS, bd as getNaturalSize, bf as updateFieldValue, cu as resolveStops, ed as runFrameSequence, ef as screenUVToBoxLocal$1, i as registerShader, iu as resolveListItems, lu as sortStops, n as getRegisteredShaders, nd as isSdfContentBoundsScanned, nu as packList, pd as maxPixelRatioForDevice, ql as colorToRGBA, qu as applyEasing, r as onShaderRegistered, ru as parseListDriverPath, su as colorStopsTransform, t as getRegisteredShader, td as getSdfContentBounds, tp as warn, tu as listPropTransform, vf as FieldHandle, xd as onNaturalSizeChange, xf as typegpu_default, yf as createUniformStore, zl as bindingSizePx, zu as SHAPE3D_DEFAULTS } from "./std-ClftfWpF.js";
import { a as getGpuUnusableReason, c as isGpuUnavailableError, f as markGpuUnusable, i as debugWarn, l as isShadersDebug, n as authorError, o as getWebGPUSupport, p as setShadersDebug, r as debugError, s as hasCustomWgsl, t as GpuUnavailableError, u as isWebGPUSupported } from "./support-D3W86vgR.js";
import "./Neon-4PemK9Rj.js";
import "./Emboss-DMNFb_ww.js";
import "./Form3D-DQDOHWpC.js";
import "./ThinFilm-D7R2dt1O.js";
import { o as transformColorSpace } from "./transformations-P50Uo_Ju.js";
import "./AngularBlur-BEBs-lRR.js";
import "./Arc-D-WnjM7T.js";
import "./Ascii-BGPIwMc3.js";
import "./Aurora-DqJBPdMQ.js";
import "./BarnDoors-B9B-AmCV.js";
import "./BarShift-BcCOweUh.js";
import "./Beam-QG_9OS8I.js";
import "./Bend-Cy9VAowL.js";
import "./Blob-C3txJeW0.js";
import "./BlockDissolve-C1SFUys7.js";
import "./noiseColor-D_bPhJ4m.js";
import "./BlockNoise-Cm3y8hce.js";
import "./BlueNoise-BCLObOMm.js";
import "./Blur-Dg29SnXd.js";
import "./Boids-rsGsPN5h.js";
import "./BokehBlur-rFSFEnb0.js";
import "./BrickPattern-DqN6JKEN.js";
import "./BrightnessContrast-J9fCSBiC.js";
import "./BrushedMetal-74wSZKjG.js";
import "./Bulge-BIk0oq8j.js";
import "./CarbonFiber-CZdUDvRU.js";
import "./Chalkboard-CClJHqvs.js";
import "./ChannelBlur-BEuEwQOC.js";
import "./Checkerboard-Rq9ze1W3.js";
import "./CheckerWipe-KuAWj2qg.js";
import "./Chevron-C1j6X2Ye.js";
import "./ChromaFlow-BgsCUYIl.js";
import "./ChromaticAberration-2_4tjQ0u.js";
import "./Chrome-Be3z6C5i.js";
import "./Circle-e_qQI3tU.js";
import "./ColorWheel-s5ep1H7d.js";
import "./CompressionArtifacts-OE9U9Qrq.js";
import "./ConcentricSpin-CLE_5yCJ.js";
import "./ConicGradient-Cji2SgVO.js";
import "./ContourLines-wpPK00cF.js";
import "./CornerPin-gYU0Uf2m.js";
import "./Crescent-viNc7Unp.js";
import "./Cross-DyQSsw_k.js";
import "./CRTScreen-hEH-JNRd.js";
import "./Crystal-Vjj8azWg.js";
import "./CurlNoise-C7PHjDht.js";
import "./CursorRipples-BzPPNkA3.js";
import "./CursorTrail-cBj0AuQA.js";
import "./DataMosh-5Yr9TyVx.js";
import "./DiamondGradient-DZM7HwF5.js";
import "./DiamondWipe-CqeT7lCt.js";
import "./DiffuseBlur-Y5hSDTZF.js";
import "./DisplacementMap-FobRx5Ie.js";
import "./Dither-50rqHpt9.js";
import "./DotGrid-B6AGLUbR.js";
import "./DropShadow-CEPpgatB.js";
import "./Duotone-D76Kp7Pb.js";
import "./Ellipse-BGod1LeG.js";
import "./Engraving-Dj7bCATQ.js";
import "./ErosionNoise-4O68wJQZ.js";
import "./Exposure-h-vapOpj.js";
import "./FallingLines-Dzop_89k.js";
import "./FilmGrain-Bs8PCfdf.js";
import "./FilmStock-DBjVo6rL.js";
import "./Flip-CfrS4KfZ.js";
import "./FloatingParticles-D1R2tD2J.js";
import "./Flower-Ddlut7QZ.js";
import "./FlowField-Slirm5AI.js";
import "./FlowingGradient-CwIgTtbj.js";
import "./FlutedGlass-CMMy2GJ5.js";
import "./Fog-DjjUkcTZ.js";
import "./FractalNoise-BjI3IOIj.js";
import "./Frost-CK12HnL1.js";
import "./GaborNoise-DKEIvvRJ.js";
import "./Glass-CzcOVD5Y.js";
import "./GlassTiles-gjhc563g.js";
import "./Glitch-C_XAWltC.js";
import "./Glow-CrYCanBI.js";
import "./Godrays-BBTMOVLP.js";
import "./Goo-Brhjk8aS.js";
import "./GradientMap-C5325QWw.js";
import "./Grayscale-dPjs-wvf.js";
import "./Grid-UjEd3O76.js";
import "./GridDistortion-vVgYya1U.js";
import "./Group-KVdmAi5g.js";
import "./Halftone-pRV3TRUW.js";
import "./Heart-CX7MvvHZ.js";
import "./Heatmap-Dn6qyd2e.js";
import "./HexGrid-BIfYhkfV.js";
import "./Hologram-ou27baPQ.js";
import "./Holographic-CiHdBSMg.js";
import "./HTMLInCanvas-CKr2u76R.js";
import "./HueShift-CG7pcXfg.js";
import "./ImageTexture-BiZuU58C.js";
import "./InkFlow-D9_L7opB.js";
import "./Invert-CHEaiQZO.js";
import "./IrisWipe-x22vOQHm.js";
import "./Irradiance-7yq2K87_.js";
import "./IsometricCubes-CkPGWr7M.js";
import "./Kaleidoscope-CEC5m1Ww.js";
import "./KeyFrames-BMGEZsAZ.js";
import "./LensDistortion-DKo8PiO5.js";
import "./LensFlare-DMS-NUtK.js";
import "./LightEdge-DxmnCRls.js";
import "./LightLeak-yoEsZtYg.js";
import "./Line-D17XDUjO.js";
import "./LinearBlur-CiyjPTUH.js";
import "./LinearGradient-DfjKG70q.js";
import "./LinearWipe-D9CtBg6S.js";
import "./LiquidMetal-8EG6DwV6.js";
import "./Liquify-BySXF1OB.js";
import "./MagneticFilings-DNESH3Zs.js";
import "./Marble-C9WvdxrP.js";
import "./MeshGradient-d9--RXOU.js";
import "./Mirror-1V2FF2kD.js";
import "./MultiPointGradient-cRA9ytgL.js";
import "./Nebula-BRV-q74y.js";
import "./NoiseDissolve-B7LgD46G.js";
import "./ObjectTracker-CdzDxhpy.js";
import "./Obsidian-2J1eX9kT.js";
import "./PagePeel-D-ziraOm.js";
import "./Paper-BxzCLyzB.js";
import "./Parallelogram-D4OflTwn.js";
import "./ParticleField-tWNicEDO.js";
import "./ParticleFlow-BiEYFyWR.js";
import "./Particles-B7W4yWgo.js";
import "./PerlinNoise-C-irUxp9.js";
import "./Perspective-DlnAW-pe.js";
import "./Pixelate-CZdwB--K.js";
import "./PixelSort-BBOTYuOE.js";
import "./PixelThrow-DCJb9Lxd.js";
import "./Plasma-CTOQJeqy.js";
import "./Plastic-DvyH9x-4.js";
import "./PolarCoordinates-XhdDAOD0.js";
import "./Polygon-CLohB-Ly.js";
import "./Posterize-BAQfnvmK.js";
import "./Prism-DJVCVAXX.js";
import "./ProgressiveBlur-Dirva1on.js";
import "./RadialGradient-DuSKR8Ds.js";
import "./RadialWipe-n8tON7x2.js";
import "./RandomBars-BYouiIgQ.js";
import "./ReactionDiffusion-CHDd3y2q.js";
import "./RectangularCoordinates-CYFfCawa.js";
import "./ReflectivePlane-DFTl5U9q.js";
import "./Repeater-CnbzXuAe.js";
import "./Ring-CGkHVSVd.js";
import "./Ripples-C2tITU62.js";
import "./RippleWipe-DLjPzY6u.js";
import "./RoundedRect-sfNTUaCY.js";
import "./Saturation-BRkifYWj.js";
import "./Scratches-D87O5C2K.js";
import "./Sharpness-BGv613Zw.js";
import "./Shatter-DuyCFU1f.js";
import "./SimplexNoise-CQJvmqd-.js";
import "./SineWave-BfOgSGMP.js";
import "./SliceWipe-CD_D99sl.js";
import "./Smoke-BDQYKRP1.js";
import "./SmokeFill-CPnzQtQv.js";
import "./SmokeFlow-BkPMjZq8.js";
import "./Solarize-DSiS32QS.js";
import "./SolidColor-DEKpgy1c.js";
import "./Sparkle-BZur6gPW.js";
import "./Spherize-D9Yg4n6O.js";
import "./Spiral-BCacfEpT.js";
import "./Star-Dnw1ihk7.js";
import "./Stone-Cje-_vAg.js";
import "./Strands-CFRxS1G_.js";
import "./Stretch-DXVhfIqO.js";
import "./Stripes-CCADWwyq.js";
import "./StudioBackground-BlzYNAsu.js";
import "./SunBurst-CmWbI18r.js";
import "./Surface3D-Dmh98-0V.js";
import "./Swirl-CzI1qSVc.js";
import "./Teardrop-D24uulL0.js";
import { a as wrapLines, i as measureTextBlock, r as measureText } from "./Text-CrZKvBw6.js";
import "./TiltShift-AMMDZBvf.js";
import "./TimeTrail-BCYDP2ep.js";
import "./Tint-DHb5wBN5.js";
import "./Trapezoid-CL5Fxvq7.js";
import "./TriangularGrid-Bp-SJS-H.js";
import "./Tritone-C0nx7OdC.js";
import "./Truchet-DCetyDaq.js";
import "./Twirl-D_mv9KEi.js";
import "./VenetianBlinds-DUyvaT5w.js";
import "./Vesica-BpzrTPTA.js";
import "./VHS-Cp9aDjti.js";
import "./Vibrance-DW3J-pvW.js";
import "./VideoTexture-zoKsSrex.js";
import "./Vignette-KKbP3qdZ.js";
import "./Voronoi-Bp2-U6lm.js";
import "./Voxels-B3e3LHH5.js";
import "./Water-CoYwvqKx.js";
import "./Watercolor-DUNjSIN1.js";
import "./WaveDistortion-Dp8ws0z1.js";
import "./Waveform-DDBLB6zu.js";
import "./WaveletNoise-CpBkzfRT.js";
import "./Weave-cWkvSEAy.js";
import "./WebcamTexture-D484SXSE.js";
import "./Wool-C9L7O9fD.js";
import "./WorleyNoise-B5j5lDQZ.js";
import "./ZoomBlur-bPC1dBu2.js";
import { n as getShaderByName, t as getAllShaders } from "./shaderRegistry-BaozBFyh.js";
const fullScreenTriangle = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(vertexFn({
	in: { vertexIndex: builtin.vertexIndex },
	out: {
		pos: builtin.position,
		uv: vec2f
	}
})`{
  const pos = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
  const uv = array<vec2f, 3>(vec2f(0, 1), vec2f(2, 1), vec2f(0, -1));

  return Out(vec4f(pos[in.vertexIndex], 0, 1), uv[in.vertexIndex]);
}`, "fullScreenTriangle");
if ("production" !== "development" && "production" !== "test") warn.disable("external-omitted");
var OPTIONAL_FEATURES = ["float32-filterable", "timestamp-query"];
var rootsByDevice = /* @__PURE__ */ new WeakMap();
var lostCallbacks = /* @__PURE__ */ new WeakMap();
var lostDevices = /* @__PURE__ */ new WeakSet();
var defaultRootPromise = null;
var destroyedDefaultRoots = /* @__PURE__ */ new WeakSet();
async function acquireRoot(options = {}) {
	if (options.device && !lostDevices.has(options.device)) {
		const device = options.device;
		let root = rootsByDevice.get(device);
		if (!root) {
			root = typegpu_default.initFromDevice({ device });
			rootsByDevice.set(device, root);
		}
		const release$1 = wireDeviceLost(root, options.onDeviceLost);
		return {
			root,
			device,
			adapter: options.adapter,
			createdDevice: false,
			release: release$1
		};
	}
	if (options.device) debugWarn("[gpu] injected GPU device was lost — falling back to the shared default device");
	const ctx = await acquireDefaultRoot();
	const release = wireDeviceLost(ctx.root, options.onDeviceLost);
	return {
		root: ctx.root,
		device: ctx.device,
		adapter: ctx.adapter,
		createdDevice: false,
		release
	};
}
async function acquireDefaultRoot() {
	for (;;) {
		const cached = defaultRootPromise;
		if (!cached) break;
		const ctx = await cached.catch(() => null);
		if (ctx && !destroyedDefaultRoots.has(cached) && !lostDevices.has(ctx.device)) return ctx;
		if (defaultRootPromise === cached) defaultRootPromise = null;
	}
	const pending = createOwnRoot();
	defaultRootPromise = pending;
	try {
		return await pending;
	} catch (error) {
		if (defaultRootPromise === pending) defaultRootPromise = null;
		throw error;
	}
}
function destroyDefaultGpuDevice() {
	const pending = defaultRootPromise;
	defaultRootPromise = null;
	if (!pending) return;
	destroyedDefaultRoots.add(pending);
	pending.then((ctx) => {
		lostDevices.add(ctx.device);
		try {
			ctx.root.destroy();
		} catch {}
	}).catch(() => {});
}
async function createOwnRoot() {
	if (typeof navigator === "undefined" || !navigator.gpu) throw new GpuUnavailableError("unsupported", "[gpu] WebGPU is not available in this environment (navigator.gpu is undefined).");
	let root;
	try {
		root = await typegpu_default.init({
			adapter: { powerPreference: "high-performance" },
			device: { optionalFeatures: OPTIONAL_FEATURES }
		});
	} catch (error) {
		const info = await getWebGPUSupport();
		throw new GpuUnavailableError(info.supported ? "no-device" : info.reason ?? "init-failed", void 0, { cause: error });
	}
	if (!root.device) throw new GpuUnavailableError("no-device", "[gpu] adapter returned no device.");
	rootsByDevice.set(root.device, root);
	return {
		root,
		device: root.device,
		createdDevice: true
	};
}
function configureCanvasContext(root, canvas, options = {}) {
	return root.configureContext({
		canvas,
		alphaMode: "premultiplied",
		...options
	});
}
function wireDeviceLost(root, onDeviceLost) {
	const device = root.device;
	let callbacks = lostCallbacks.get(device);
	if (!callbacks) {
		callbacks = /* @__PURE__ */ new Set();
		lostCallbacks.set(device, callbacks);
		root.device.lost.then((info) => {
			lostDevices.add(device);
			if (info.reason === "destroyed") return;
			for (const cb of [...lostCallbacks.get(device) ?? []]) cb(info);
		});
	}
	if (!onDeviceLost) return () => {};
	const set = callbacks;
	set.add(onDeviceLost);
	return () => set.delete(onDeviceLost);
}
function createAnimatedTimeState() {
	const state = {
		value: 0,
		advance(deltaTime, speed) {
			state.value += deltaTime * speed;
		}
	};
	return state;
}
var accumulatedTimeCache = /* @__PURE__ */ new WeakMap();
function getAnimatedTimeState(key) {
	let state = accumulatedTimeCache.get(key);
	if (!state) {
		state = createAnimatedTimeState();
		accumulatedTimeCache.set(key, state);
	}
	return state;
}
const SystemUniforms = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	time: f32,
	viewportSize: vec2f$1,
	logicalViewportSize: vec2f$1,
	aspect: f32,
	pointer: vec2f$1,
	pointerActive: f32
}), "SystemUniforms");
typegpu_default.fn([vec2f$1, vec2f$1], vec2f$1)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((uv, viewportSize) => {
	"use gpu";
	return mul(uv, viewportSize);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [{
			"type": "i",
			"name": "uv"
		}, {
			"type": "i",
			"name": "viewportSize"
		}],
		"body": [0, [[10, [
			6,
			"std.mul",
			["uv", "viewportSize"]
		]]]]
	},
	externals: { "std.mul": () => mul }
}) && $.f)({}));
typegpu_default.fn([vec2f$1], f32)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((viewportSize) => {
	"use gpu";
	return __tsover_div(viewportSize.x, viewportSize.y);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [{
			"type": "i",
			"name": "viewportSize"
		}],
		"body": [0, [[10, [
			1,
			[
				7,
				"viewportSize",
				"x"
			],
			"/",
			[
				7,
				"viewportSize",
				"y"
			]
		]]]]
	},
	externals: {}
}) && $.f)({}));
function structuralHash(inputs) {
	let h = 2166136261;
	const joined = inputs.join("");
	for (let i = 0; i < joined.length; i++) {
		h ^= joined.charCodeAt(i);
		h = Math.imul(h, 16777619);
	}
	return `${(h >>> 0).toString(16).padStart(8, "0")}:${joined.length}`;
}
function createPipelineCache(options = {}) {
	const maxSize = options.maxSize ?? 4;
	const dispose = options.dispose;
	const entries = /* @__PURE__ */ new Map();
	let clock = 0;
	let activeHash = null;
	let pendingHash = null;
	function touch(entry) {
		entry.lastUsed = ++clock;
	}
	function evictIfNeeded() {
		while (entries.size > maxSize) {
			let victim = null;
			for (const e of entries.values()) {
				if (e.hash === activeHash || e.hash === pendingHash) continue;
				if (!victim || e.lastUsed < victim.lastUsed) victim = e;
			}
			if (!victim) break;
			entries.delete(victim.hash);
			dispose?.(victim.value, victim.hash);
		}
	}
	return {
		getOrBuild(hash, buildFn) {
			const existing = entries.get(hash);
			if (existing) {
				touch(existing);
				if (hash === activeHash) pendingHash = null;
				else pendingHash = hash;
				return existing.value;
			}
			const value = buildFn();
			const entry = {
				hash,
				value,
				lastUsed: ++clock
			};
			entries.set(hash, entry);
			if (activeHash === null) {
				activeHash = hash;
				pendingHash = null;
			} else pendingHash = hash;
			evictIfNeeded();
			return value;
		},
		markReady(hash) {
			if (hash !== pendingHash) return;
			const entry = entries.get(hash);
			if (!entry) return;
			activeHash = hash;
			pendingHash = null;
			touch(entry);
			evictIfNeeded();
		},
		get renderValue() {
			const hash = activeHash ?? pendingHash;
			return hash ? entries.get(hash)?.value ?? null : null;
		},
		get activeHash() {
			return activeHash;
		},
		get pendingHash() {
			return pendingHash;
		},
		release(hash) {
			const entry = entries.get(hash);
			if (!entry) return;
			entries.delete(hash);
			dispose?.(entry.value, hash);
			if (activeHash === hash) activeHash = null;
			if (pendingHash === hash) pendingHash = null;
		},
		has(hash) {
			return entries.has(hash);
		},
		get size() {
			return entries.size;
		},
		clear() {
			for (const e of entries.values()) dispose?.(e.value, e.hash);
			entries.clear();
			activeHash = null;
			pendingHash = null;
		}
	};
}
var LABEL = "passManager";
function createPassManager(root, options) {
	const { textureManager, dispatcher } = options;
	const canvasFormat = options.canvasFormat ?? preferredCanvasFormat();
	const rttFormat = options.rttFormat ?? "rgba16float";
	const deviceMaxSampled = root.device?.limits?.maxSampledTexturesPerShaderStage;
	const maxSampled = options.maxSampledTextures ?? (typeof deviceMaxSampled === "number" && deviceMaxSampled > 0 ? deviceMaxSampled : 16);
	const vertex = options.fullScreenTriangle ?? fullScreenTriangle;
	const rttTextures = /* @__PURE__ */ new Map();
	const liveTextures = /* @__PURE__ */ new Set();
	let current = null;
	function buildSamplerBindGroup() {
		const s = textureManager.samplers;
		try {
			return root.createBindGroup(SHARED_SAMPLER_LAYOUT, {
				linearClamp: s.linearClamp,
				nearestClamp: s.nearestClamp,
				linearRepeat: s.linearRepeat,
				nearestRepeat: s.nearestRepeat
			});
		} catch (e) {
			debugError(`[${LABEL}] failed to build sampler bind group:`, e);
			return;
		}
	}
	function allocateRttTextures(ir, size) {
		const needed = /* @__PURE__ */ new Set();
		for (const t of ir.textures) {
			if (t.kind !== "rtt") continue;
			needed.add(t.key);
			if (!rttTextures.has(t.key)) {
				const rt = textureManager.createRenderTexture({
					width: size.width,
					height: size.height,
					format: rttFormat,
					label: `rtt:${t.key}`
				});
				rttTextures.set(t.key, rt);
				liveTextures.add(rt);
			} else rttTextures.get(t.key).resize(size.width, size.height);
		}
		for (const [key, rt] of [...rttTextures]) if (!needed.has(key)) {
			rt.destroy();
			liveTextures.delete(rt);
			rttTextures.delete(key);
		}
	}
	function buildTextureBindGroup(fragment, mediaResources) {
		if (!fragment.textureLayout || fragment.reads.length === 0) return void 0;
		const entries = {};
		for (const key of fragment.reads) {
			const rt = rttTextures.get(key);
			const media = mediaResources?.[key];
			const resource = rt ?? media;
			if (!resource) {
				debugError(`[${LABEL}] pass "${fragment.entry}" reads unknown texture "${key}"`);
				return;
			}
			entries[key] = resource.texture;
		}
		if (fragment.reads.length > maxSampled) debugWarn(`[${LABEL}] pass samples ${fragment.reads.length} textures (> device limit ~${maxSampled}); composition may fail on this device.`);
		try {
			return root.createBindGroup(fragment.textureLayout, entries);
		} catch (e) {
			debugError(`[${LABEL}] failed to build texture bind group:`, e);
			return;
		}
	}
	function buildPipeline(fragment, targetFormat) {
		return root.createRenderPipeline({
			vertex,
			fragment: fragment.entry,
			targets: { format: targetFormat }
		});
	}
	function buildExternalBinding(fragment, externalSources) {
		if (!fragment.externalLayout || fragment.externalReads.length === 0) return void 0;
		const keys = fragment.externalReads;
		if (keys.length === 1) {
			const getSource = externalSources.get(keys[0]);
			if (!getSource) {
				debugError(`[${LABEL}] external read "${keys[0]}" has no registered source`);
				return;
			}
			return textureManager.createExternalTextureBinding({
				layout: fragment.externalLayout,
				entryKey: keys[0],
				getSource
			});
		}
		let current$1 = null;
		return {
			update() {
				const entries = {};
				for (const key of keys) {
					const source = externalSources.get(key)?.();
					if (!source) {
						current$1 = null;
						return null;
					}
					try {
						entries[key] = root.device.importExternalTexture({ source });
					} catch {
						current$1 = null;
						return null;
					}
				}
				current$1 = root.createBindGroup(fragment.externalLayout, entries);
				return current$1;
			},
			get current() {
				return current$1;
			},
			destroy() {
				current$1 = null;
			}
		};
	}
	function buildExecutablePass(fragment, target, mediaResources, externalSources) {
		const targetFormat = target.kind === "canvas" ? canvasFormat : rttFormat;
		const mediaReads = fragment.reads.filter((k) => mediaResources?.[k]);
		const mediaSnapshot = new Map(mediaReads.map((k) => [k, mediaResources[k].texture]));
		return {
			fragment,
			pipeline: buildPipeline(fragment, targetFormat),
			textureBindGroup: buildTextureBindGroup(fragment, mediaResources),
			externalBinding: buildExternalBinding(fragment, externalSources),
			target,
			mediaReads,
			mediaSnapshot
		};
	}
	function setComposition(ir, uniformBindGroup, size, mediaResources) {
		allocateRttTextures(ir, size);
		for (const spec of ir.computeSteps) spec.bindInputs?.((key) => rttTextures.get(key) ?? mediaResources?.[key]);
		const samplerBindGroup = buildSamplerBindGroup();
		const externalSources = /* @__PURE__ */ new Map();
		for (const e of ir.externalTextures) externalSources.set(e.key, e.getSource);
		const rttPasses = ir.rttPasses.map((p) => buildExecutablePass(p.fragment, {
			kind: "rtt",
			key: p.textureKey
		}, mediaResources, externalSources));
		const finalPass = buildExecutablePass(ir.finalPass, { kind: "canvas" }, mediaResources, externalSources);
		current = {
			ir,
			uniformBindGroup,
			samplerBindGroup,
			computeSteps: ir.computeSteps,
			rttPasses,
			finalPass,
			size,
			mediaResources
		};
	}
	function execPass(pass, view) {
		let p = pass.pipeline;
		if (current?.uniformBindGroup) p = p.with(current.uniformBindGroup);
		if (pass.mediaReads && pass.mediaReads.length > 0 && current?.mediaResources) {
			let swapped = false;
			for (const key of pass.mediaReads) if (current.mediaResources[key].texture !== pass.mediaSnapshot.get(key)) swapped = true;
			if (swapped) {
				pass.textureBindGroup = buildTextureBindGroup(pass.fragment, current.mediaResources);
				for (const key of pass.mediaReads) pass.mediaSnapshot.set(key, current.mediaResources[key].texture);
			}
		}
		if (pass.textureBindGroup) p = p.with(pass.textureBindGroup);
		if (pass.fragment.usesSamplers && current?.samplerBindGroup) p = p.with(current.samplerBindGroup);
		if (pass.fragment.externalReads.length > 0) {
			const bg = pass.externalBinding?.update() ?? null;
			if (!bg) {
				if (!pass.externalSkipLogged) {
					debugWarn(`[${LABEL}] external texture not ready — skipping pass (${pass.fragment.externalReads.join(", ")})`);
					pass.externalSkipLogged = true;
				}
				return;
			}
			p = p.with(bg);
		}
		p.withColorAttachment({ view }).draw(3);
	}
	function render(canvasView, frameParams, afterCompute) {
		if (!current) return;
		for (const step of current.computeSteps) {
			const steps = step.getComputeNodes(frameParams);
			if (steps && steps.length) dispatcher.dispatch(steps);
		}
		afterCompute?.();
		repaintRtt();
		execPass(current.finalPass, canvasView);
	}
	function repaintRtt() {
		if (!current) return;
		for (const pass of current.rttPasses) {
			const rt = rttTextures.get(pass.target.key);
			if (!rt) continue;
			execPass(pass, renderView(rt));
		}
	}
	function renderView(rt) {
		return root.unwrap(rt.texture).createView();
	}
	function rebindPassTextures(pass) {
		if (!pass.fragment.textureLayout || pass.fragment.reads.length === 0) return;
		pass.textureBindGroup = buildTextureBindGroup(pass.fragment, current?.mediaResources);
		if (pass.mediaReads && pass.mediaSnapshot && current?.mediaResources) for (const key of pass.mediaReads) pass.mediaSnapshot.set(key, current.mediaResources[key].texture);
	}
	function resize(width, height) {
		let recreated = false;
		for (const rt of rttTextures.values()) if (rt.resize(width, height)) recreated = true;
		if (!current) return recreated;
		current.size = {
			width,
			height
		};
		if (!recreated) return false;
		for (const spec of current.computeSteps) spec.bindInputs?.((key) => rttTextures.get(key) ?? current.mediaResources?.[key]);
		for (const pass of current.rttPasses) rebindPassTextures(pass);
		rebindPassTextures(current.finalPass);
		return true;
	}
	return {
		setComposition,
		render,
		repaintRtt,
		resize,
		get textureCount() {
			return liveTextures.size;
		},
		get passCount() {
			return current ? current.rttPasses.length + 1 : 0;
		},
		dispose() {
			for (const rt of [...liveTextures]) rt.destroy();
			liveTextures.clear();
			rttTextures.clear();
			current = null;
		}
	};
}
function preferredCanvasFormat() {
	if (typeof navigator !== "undefined" && navigator.gpu) return navigator.gpu.getPreferredCanvasFormat();
	return "bgra8unorm";
}
function srgbViewFormat(format) {
	return `${format}-srgb`;
}
function createTextureManager(root) {
	const live = /* @__PURE__ */ new Set();
	function createRaw(props, ...usages) {
		return root.createTexture.call(root, props).$usage(...usages);
	}
	let linearClamp;
	let nearestClamp;
	let linearRepeat;
	let nearestRepeat;
	const samplers = {
		get linearClamp() {
			return linearClamp ??= root.createSampler({
				magFilter: "linear",
				minFilter: "linear",
				mipmapFilter: "linear",
				addressModeU: "clamp-to-edge",
				addressModeV: "clamp-to-edge"
			});
		},
		get nearestClamp() {
			return nearestClamp ??= root.createSampler({
				magFilter: "nearest",
				minFilter: "nearest",
				addressModeU: "clamp-to-edge",
				addressModeV: "clamp-to-edge"
			});
		},
		get linearRepeat() {
			return linearRepeat ??= root.createSampler({
				magFilter: "linear",
				minFilter: "linear",
				mipmapFilter: "linear",
				addressModeU: "repeat",
				addressModeV: "repeat"
			});
		},
		get nearestRepeat() {
			return nearestRepeat ??= root.createSampler({
				magFilter: "nearest",
				minFilter: "nearest",
				addressModeU: "repeat",
				addressModeV: "repeat"
			});
		}
	};
	function wrap(texture, width, height, format, srgb) {
		const managed = {
			texture,
			width,
			height,
			format,
			write(source) {
				texture.write(source);
			},
			unwrap() {
				return root.unwrap(texture);
			},
			createSrgbView() {
				if (!srgb) return void 0;
				return root.unwrap(texture).createView({ format: srgbViewFormat(format) });
			},
			generateMipmaps() {
				texture.generateMipmaps?.();
			},
			destroy() {
				if (!live.has(managed)) return;
				texture.destroy();
				live.delete(managed);
			}
		};
		live.add(managed);
		return managed;
	}
	function createMediaTexture(options) {
		const { width, height, srgb = false, mipLevelCount } = options;
		const format = options.format ?? "rgba8unorm";
		return wrap(createRaw({
			size: [width, height],
			format,
			...srgb ? { viewFormats: [srgbViewFormat(format)] } : {},
			...mipLevelCount ? { mipLevelCount } : {}
		}, "sampled", "render"), width, height, format, srgb);
	}
	function createDataTexture(options) {
		const { width, height, format, data } = options;
		const managed = wrap(createRaw({
			size: [width, height],
			format
		}, "sampled"), width, height, format, false);
		if (data) managed.write(data);
		return managed;
	}
	function createRenderTexture(options) {
		const format = options.format ?? "rgba16float";
		const { mipLevelCount } = options;
		let width = options.width;
		let height = options.height;
		function build(w, h) {
			return wrap(createRaw({
				size: [w, h],
				format,
				...mipLevelCount ? { mipLevelCount } : {}
			}, "sampled", "render"), w, h, format, false);
		}
		let inner = build(width, height);
		return {
			get texture() {
				return inner.texture;
			},
			get width() {
				return width;
			},
			get height() {
				return height;
			},
			format,
			write(source) {
				inner.write(source);
			},
			unwrap() {
				return inner.unwrap();
			},
			createSrgbView() {
				return inner.createSrgbView();
			},
			generateMipmaps() {
				inner.generateMipmaps();
			},
			resize(w, h) {
				if (w === width && h === height) return false;
				inner.destroy();
				width = w;
				height = h;
				inner = build(w, h);
				return true;
			},
			destroy() {
				inner.destroy();
			}
		};
	}
	function createExternalTextureBinding(options) {
		const { layout, entryKey, getSource, staticEntries = {} } = options;
		let current = null;
		return {
			update() {
				const source = getSource();
				if (!source) {
					current = null;
					return null;
				}
				let external;
				try {
					external = root.device.importExternalTexture({ source });
				} catch {
					current = null;
					return null;
				}
				current = root.createBindGroup(layout, {
					...staticEntries,
					[entryKey]: external
				});
				return current;
			},
			get current() {
				return current;
			},
			destroy() {
				current = null;
			}
		};
	}
	return {
		samplers,
		createMediaTexture,
		createDataTexture,
		createRenderTexture,
		createExternalTextureBinding,
		get textureCount() {
			return live.size;
		},
		destroy() {
			for (const t of [...live]) t.destroy();
			live.clear();
		}
	};
}
function mediaKey(type, props) {
	const decl = (getShaderByName(type)?.definition)?.naturalSizeKey;
	if (!decl) return null;
	if ("fixed" in decl) return decl.fixed || null;
	const value = props[decl.fromProp];
	return typeof value === "string" && value ? value : null;
}
function explicitBoxSize(bbox, cw, ch) {
	if (!bbox || typeof bbox !== "object") return null;
	const dimPx = (d, dim) => {
		if (!d || typeof d !== "object" || typeof d.value !== "number") return null;
		return d.unit === "px" ? d.value : d.value * dim;
	};
	const w = dimPx(bbox.width, cw);
	const h = dimPx(bbox.height, ch);
	return w != null && h != null && w > 0 && h > 0 ? {
		widthPx: w,
		heightPx: h
	} : null;
}
function measureNode(input, cw, ch) {
	const { type, decl, props, boundingBox } = input;
	if (!(cw > 0) || !(ch > 0)) return null;
	if (input.requiresChild) return null;
	if (decl?.computeBounds) {
		const b = decl.computeBounds(props, cw, ch);
		if (b.widthPx > 0 && b.heightPx > 0) return {
			widthPx: b.widthPx,
			heightPx: b.heightPx
		};
		return null;
	}
	const pb = decl?.propBindings;
	if (pb?.width && pb?.height) {
		const w = bindingSizePx(pb.width, props[pb.width.prop], cw, ch);
		const h = bindingSizePx(pb.height, props[pb.height.prop], cw, ch);
		if (w > 0 && h > 0) return {
			widthPx: w,
			heightPx: h
		};
		return null;
	}
	const boxed = explicitBoxSize(boundingBox, cw, ch);
	if (boxed) return boxed;
	const key = mediaKey(type, props);
	if (key) {
		const natural = getNaturalSize(key);
		if (natural) return {
			widthPx: natural.width,
			heightPx: natural.height
		};
	}
	return null;
}
function gapToPx(gap, mainAxisDim) {
	if (gap == null) return 0;
	if (typeof gap === "number") return gap;
	return gap.unit === "px" ? gap.value : gap.value * mainAxisDim;
}
function resolveLayoutConfig(cfg, cw, ch) {
	if (!cfg || cfg.mode !== "column" && cfg.mode !== "row") return null;
	return {
		mode: cfg.mode,
		gapPx: Math.max(0, gapToPx(cfg.gap, cfg.mode === "column" ? ch : cw)),
		align: cfg.align ?? "center",
		anchor: cfg.anchor ?? "center",
		anchorOffsetX: cfg.anchorOffset?.x ?? 0,
		anchorOffsetY: cfg.anchorOffset?.y ?? 0
	};
}
var anchorEdges = (origin) => ({
	ox: origin.includes("left") ? "start" : origin.includes("right") ? "end" : "center",
	oy: origin.includes("top") ? "start" : origin.includes("bottom") ? "end" : "center"
});
var anchorTopLeft = (edge, blockSize, canvasDim, offsetPx) => edge === "start" ? offsetPx : edge === "end" ? canvasDim - blockSize - offsetPx : (canvasDim - blockSize) / 2 + offsetPx;
function layoutBlockSize(layout, items) {
	let main = 0;
	let cross = 0;
	for (const item of items) {
		const m = layout.mode === "column" ? item.size.heightPx : item.size.widthPx;
		const c = layout.mode === "column" ? item.size.widthPx : item.size.heightPx;
		main += m;
		cross = Math.max(cross, c);
	}
	if (items.length > 1) main += layout.gapPx * (items.length - 1);
	return layout.mode === "column" ? {
		widthPx: cross,
		heightPx: main
	} : {
		widthPx: main,
		heightPx: cross
	};
}
function computeLayout(layout, items, cw, ch) {
	const placements = /* @__PURE__ */ new Map();
	if (items.length === 0) return placements;
	const block = layoutBlockSize(layout, items);
	const { ox, oy } = anchorEdges(layout.anchor);
	const blockLeft = anchorTopLeft(ox, block.widthPx, cw, layout.anchorOffsetX);
	const blockTop = anchorTopLeft(oy, block.heightPx, ch, layout.anchorOffsetY);
	const crossPos = (childCross, blockCross) => layout.align === "start" ? 0 : layout.align === "end" ? blockCross - childCross : (blockCross - childCross) / 2;
	let cursor = 0;
	for (const item of items) if (layout.mode === "column") {
		const x = blockLeft + crossPos(item.size.widthPx, block.widthPx) + item.size.widthPx / 2;
		const y = blockTop + cursor + item.size.heightPx / 2;
		placements.set(item.id, {
			centerXPx: x,
			centerYPx: y
		});
		cursor += item.size.heightPx + layout.gapPx;
	} else {
		const x = blockLeft + cursor + item.size.widthPx / 2;
		const y = blockTop + crossPos(item.size.heightPx, block.heightPx) + item.size.heightPx / 2;
		placements.set(item.id, {
			centerXPx: x,
			centerYPx: y
		});
		cursor += item.size.widthPx + layout.gapPx;
	}
	return placements;
}
function measureChild(view, cw, ch) {
	const nested = resolveLayoutConfig(view.layout, cw, ch);
	if (nested) {
		const items = flowItems(view, cw, ch);
		if (items.length === 0) return null;
		return layoutBlockSize(nested, items);
	}
	return measureNode({
		type: view.componentName,
		decl: view.decl,
		props: view.props,
		boundingBox: view.boundingBox,
		requiresChild: view.requiresChild,
		availableWidthPx: cw
	}, cw, ch);
}
function flowItems(group, cw, ch) {
	const items = [];
	for (const child of group.children) {
		if (child.visible === false || child.absolute) continue;
		const size = measureChild(child, cw, ch);
		if (size) items.push({
			id: child.id,
			size,
			view: child
		});
	}
	return items;
}
function computeBoundsShift(view, cw, ch) {
	if (!view.decl?.computeBounds) return {
		x: 0,
		y: 0
	};
	const b = view.decl.computeBounds(view.props, cw, ch);
	const cx = typeof view.props.center?.x === "number" ? view.props.center.x : .5;
	const cy = typeof view.props.center?.y === "number" ? view.props.center.y : .5;
	return {
		x: b.centerXPx - cx * cw,
		y: b.centerYPx - cy * ch
	};
}
function placeChild(view, size, centerXPx, centerYPx, cw, ch, out, rects) {
	const nested = resolveLayoutConfig(view.layout, cw, ch);
	if (nested) {
		placeGroup(view, nested, cw, ch, out, {
			centerXPx,
			centerYPx
		}, rects);
		return;
	}
	const posProp = view.decl?.propBindings?.x?.prop ?? view.decl?.propBindings?.y?.prop;
	if (posProp) {
		if (!view.positionDriven) {
			const shift = computeBoundsShift(view, cw, ch);
			out.push({
				kind: "position",
				id: view.id,
				prop: posProp,
				value: {
					x: (centerXPx - shift.x) / cw,
					y: (centerYPx - shift.y) / ch
				}
			});
		}
		return;
	}
	out.push({
		kind: "box",
		id: view.id,
		xPx: centerXPx - size.widthPx / 2,
		yPx: centerYPx - size.heightPx / 2,
		widthPx: size.widthPx,
		heightPx: size.heightPx
	});
}
function placeGroup(group, layout, cw, ch, out, at, rects, precomputedItems) {
	const items = precomputedItems ?? flowItems(group, cw, ch);
	if (items.length === 0) return;
	let effective = layout;
	if (at) {
		const block = layoutBlockSize(layout, items);
		effective = {
			...layout,
			anchor: "top-left",
			anchorOffsetX: at.centerXPx - block.widthPx / 2,
			anchorOffsetY: at.centerYPx - block.heightPx / 2
		};
	}
	if (rects) {
		const block = layoutBlockSize(effective, items);
		const { ox, oy } = anchorEdges(effective.anchor);
		rects.set(group.id, {
			leftPx: anchorTopLeft(ox, block.widthPx, cw, effective.anchorOffsetX),
			topPx: anchorTopLeft(oy, block.heightPx, ch, effective.anchorOffsetY),
			widthPx: block.widthPx,
			heightPx: block.heightPx
		});
	}
	const placements = computeLayout(effective, items, cw, ch);
	for (const item of items) {
		const p = placements.get(item.id);
		if (p) placeChild(item.view, item.size, p.centerXPx, p.centerYPx, cw, ch, out, rects);
	}
}
function anchorOffsetForBlockTopLeft(group, leftPx, topPx, cw, ch) {
	const layout = resolveLayoutConfig(group.layout, cw, ch);
	if (!layout) return null;
	const items = flowItems(group, cw, ch);
	if (items.length === 0) return null;
	const block = layoutBlockSize(layout, items);
	const { ox, oy } = anchorEdges(layout.anchor);
	const invert = (edge, pos, size, dim) => edge === "start" ? pos : edge === "end" ? dim - pos - size : pos - (dim - size) / 2;
	return {
		x: invert(ox, leftPx, block.widthPx, cw),
		y: invert(oy, topPx, block.heightPx, ch)
	};
}
function computeTreeLayout(roots, cw, ch, rects) {
	const out = [];
	const walk = (view) => {
		const layout = resolveLayoutConfig(view.layout, cw, ch);
		if (layout) {
			const items = flowItems(view, cw, ch);
			placeGroup(view, layout, cw, ch, out, void 0, rects, items);
			const placed = new Set(items.map((i) => i.id));
			for (const child of view.children) {
				if (child.visible === false) continue;
				if (!placed.has(child.id)) walk(child);
			}
			return;
		}
		for (const child of view.children) if (child.visible !== false) walk(child);
	};
	for (const root of roots) if (root.visible !== false) walk(root);
	return out;
}
function computeGroupBlockRects(roots, cw, ch) {
	const rects = /* @__PURE__ */ new Map();
	computeTreeLayout(roots, cw, ch, rects);
	return rects;
}
var PerformanceTracker = class {
	frameTimesMs = [];
	frameTimesIndex = 0;
	frameTimesCount = 0;
	maxSamples = 60;
	targetFrameTime = 16.67;
	jankFrameCount = 0;
	totalFrameCount = 0;
	memorySnapshots = [];
	memorySnapshotIndex = 0;
	maxMemorySnapshots = 300;
	nodeCount = 0;
	rttNodeCount = 0;
	lastCpuTime = null;
	lastGpuTime = null;
	isRendering = false;
	lastFrameTimestamp = 0;
	frameIntervals = [];
	frameIntervalsIndex = 0;
	frameIntervalsCount = 0;
	pushToCircularBuffer(buffer, value, index, _count, maxSize) {
		if (buffer.length < maxSize) {
			buffer.push(value);
			return {
				index: buffer.length,
				count: buffer.length
			};
		}
		buffer[index % maxSize] = value;
		return {
			index: index + 1,
			count: maxSize
		};
	}
	getBufferValues(buffer, count) {
		return count <= buffer.length ? buffer.slice(0, count) : buffer;
	}
	recordFrame(frameTimeMs) {
		const result = this.pushToCircularBuffer(this.frameTimesMs, frameTimeMs, this.frameTimesIndex, this.frameTimesCount, this.maxSamples);
		this.frameTimesIndex = result.index;
		this.frameTimesCount = result.count;
		this.totalFrameCount++;
		if (frameTimeMs > this.targetFrameTime) this.jankFrameCount++;
		const now = performance.now();
		if (this.lastFrameTimestamp > 0) {
			const interval = now - this.lastFrameTimestamp;
			const intervalResult = this.pushToCircularBuffer(this.frameIntervals, interval, this.frameIntervalsIndex, this.frameIntervalsCount, this.maxSamples);
			this.frameIntervalsIndex = intervalResult.index;
			this.frameIntervalsCount = intervalResult.count;
		}
		this.lastFrameTimestamp = now;
		this.recordMemorySnapshot();
	}
	recordCpuTime(timeMs) {
		this.lastCpuTime = timeMs;
	}
	recordGpuTime(timeMs) {
		this.lastGpuTime = timeMs;
	}
	updateNodeCounts(nodeCount, rttNodeCount) {
		this.nodeCount = nodeCount;
		this.rttNodeCount = rttNodeCount;
	}
	setRendering(rendering) {
		this.isRendering = rendering;
	}
	recordMemorySnapshot() {
		if (!performance.memory) return;
		const now = performance.now();
		const bytes = performance.memory.usedJSHeapSize;
		if (this.memorySnapshots.length < this.maxMemorySnapshots) this.memorySnapshots.push({
			time: now,
			bytes
		});
		else {
			const entry = this.memorySnapshots[this.memorySnapshotIndex];
			entry.time = now;
			entry.bytes = bytes;
			this.memorySnapshotIndex = (this.memorySnapshotIndex + 1) % this.maxMemorySnapshots;
		}
	}
	calculateMemoryGrowthRate() {
		if (this.memorySnapshots.length < 2) return null;
		const len = this.memorySnapshots.length;
		const oldestIdx = len < this.maxMemorySnapshots ? 0 : this.memorySnapshotIndex;
		const newestIdx = len < this.maxMemorySnapshots ? len - 1 : (this.memorySnapshotIndex - 1 + len) % len;
		const oldest = this.memorySnapshots[oldestIdx];
		const newest = this.memorySnapshots[newestIdx];
		const timeDeltaSeconds = (newest.time - oldest.time) / 1e3;
		if (timeDeltaSeconds <= 0) return null;
		return (newest.bytes - oldest.bytes) / (1024 * 1024) / timeDeltaSeconds;
	}
	calculateStdDev(values) {
		if (values.length === 0) return 0;
		const mean = values.reduce((a, b) => a + b, 0) / values.length;
		let sumSquaredDiff = 0;
		for (let i = 0; i < values.length; i++) {
			const diff = values[i] - mean;
			sumSquaredDiff += diff * diff;
		}
		return Math.sqrt(sumSquaredDiff / values.length);
	}
	sortBuffer = [];
	calculateP99(values) {
		const len = values.length;
		if (len === 0) return 0;
		while (this.sortBuffer.length < len) this.sortBuffer.push(0);
		for (let i = 0; i < len; i++) this.sortBuffer[i] = values[i];
		for (let i = 1; i < len; i++) {
			const current = this.sortBuffer[i];
			let j = i - 1;
			while (j >= 0 && this.sortBuffer[j] > current) {
				this.sortBuffer[j + 1] = this.sortBuffer[j];
				j--;
			}
			this.sortBuffer[j + 1] = current;
		}
		const index = Math.floor(len * .99);
		return this.sortBuffer[Math.min(index, len - 1)];
	}
	calculateComplexityScore() {
		return this.nodeCount + this.rttNodeCount * 10;
	}
	calculateIntensityScore() {
		const frameTimes = this.getBufferValues(this.frameTimesMs, this.frameTimesCount);
		const frameCount = frameTimes.length;
		if (frameCount === 0) return {
			score: 0,
			label: "N/A"
		};
		const avgFrameTime = frameTimes.reduce((a, b) => a + b, 0) / frameCount;
		const frameTimeScore = Math.min(avgFrameTime / 16.67 * 100, 100);
		const complexity = this.calculateComplexityScore();
		const complexityScore = Math.min(complexity / 100 * 100, 100);
		let gpuTimeScore = 0;
		if (this.lastGpuTime !== null) gpuTimeScore = Math.min(this.lastGpuTime / 16.67 * 100, 100);
		const weights = this.lastGpuTime !== null ? {
			frame: .4,
			gpu: .4,
			complexity: .2
		} : {
			frame: .7,
			gpu: 0,
			complexity: .3
		};
		const finalScore = Math.round(frameTimeScore * weights.frame + gpuTimeScore * weights.gpu + complexityScore * weights.complexity);
		let label;
		if (finalScore <= 20) label = "Very Light";
		else if (finalScore <= 40) label = "Light";
		else if (finalScore <= 60) label = "Medium";
		else if (finalScore <= 80) label = "Heavy";
		else label = "Very Heavy";
		return {
			score: Math.min(finalScore, 100),
			label
		};
	}
	getStats(rendererInfo) {
		const frameTimes = this.getBufferValues(this.frameTimesMs, this.frameTimesCount);
		const intervals = this.getBufferValues(this.frameIntervals, this.frameIntervalsCount);
		const frameCount = frameTimes.length;
		const fps = intervals.length > 0 ? 1e3 / (intervals.reduce((a, b) => a + b, 0) / intervals.length) : 0;
		const avgFrameTime = frameCount > 0 ? frameTimes.reduce((a, b) => a + b, 0) / frameCount : 0;
		const minFrameTime = frameCount > 0 ? Math.min(...frameTimes) : 0;
		const maxFrameTime = frameCount > 0 ? Math.max(...frameTimes) : 0;
		const p99FrameTime = this.calculateP99(frameTimes);
		const stdDevFrameTime = this.calculateStdDev(frameTimes);
		const jankPercent = this.totalFrameCount > 0 ? this.jankFrameCount / this.totalFrameCount * 100 : 0;
		const memory = performance.memory;
		const memoryUsedMB = memory ? memory.usedJSHeapSize / (1024 * 1024) : null;
		const memoryGrowthRate = this.calculateMemoryGrowthRate();
		const budgetUsed = avgFrameTime > 0 ? avgFrameTime / this.targetFrameTime * 100 : 0;
		const drawCalls = rendererInfo?.render?.calls ?? null;
		const shaderPrograms = rendererInfo?.programs?.length ?? null;
		const textureCount = rendererInfo?.memory?.textures ?? null;
		const intensity = this.calculateIntensityScore();
		return {
			fps: Math.round(fps),
			avgFrameTime: Math.round(avgFrameTime * 100) / 100,
			minFrameTime: Math.round(minFrameTime * 100) / 100,
			maxFrameTime: Math.round(maxFrameTime * 100) / 100,
			p99FrameTime: Math.round(p99FrameTime * 100) / 100,
			stdDevFrameTime: Math.round(stdDevFrameTime * 100) / 100,
			jankCount: this.jankFrameCount,
			jankPercent: Math.round(jankPercent * 10) / 10,
			nodeCount: this.nodeCount,
			rttNodeCount: this.rttNodeCount,
			complexityScore: this.calculateComplexityScore(),
			memoryUsedMB: memoryUsedMB !== null ? Math.round(memoryUsedMB * 100) / 100 : null,
			memoryGrowthRate: memoryGrowthRate !== null ? Math.round(memoryGrowthRate * 1e3) / 1e3 : null,
			cpuTime: this.lastCpuTime !== null ? Math.round(this.lastCpuTime * 100) / 100 : null,
			gpuTime: this.lastGpuTime !== null ? Math.round(this.lastGpuTime * 100) / 100 : null,
			budgetUsed: Math.round(budgetUsed * 10) / 10,
			drawCalls,
			shaderPrograms,
			textureCount,
			intensityScore: intensity.score,
			intensityLabel: intensity.label,
			isRendering: this.isRendering
		};
	}
	reset() {
		this.frameTimesMs.length = 0;
		this.frameTimesIndex = 0;
		this.frameTimesCount = 0;
		this.frameIntervals.length = 0;
		this.frameIntervalsIndex = 0;
		this.frameIntervalsCount = 0;
		this.lastFrameTimestamp = 0;
		this.jankFrameCount = 0;
		this.totalFrameCount = 0;
		this.memorySnapshots.length = 0;
		this.memorySnapshotIndex = 0;
		this.sortBuffer.length = 0;
		this.lastCpuTime = null;
		this.lastGpuTime = null;
	}
};
var MAX_UNCAPTURED_VALIDATION_ERRORS = 32;
var MAX_CONSECUTIVE_RENDER_ERRORS = 5;
var globalEventManager = (() => {
	let instanceCount = 0;
	const callbacks = /* @__PURE__ */ new Set();
	const fan = (key, arg) => {
		callbacks.forEach((cb) => {
			try {
				cb[key](arg);
			} catch (error) {
				debugError("[Shaders] Error in pointer handler:", error);
			}
		});
	};
	const onMove = (e) => fan("onMouseMove", e);
	const onTouch = (e) => fan("onTouchMove", e);
	const onDown = (e) => fan("onMouseDown", e);
	const onStart = (e) => fan("onTouchStart", e);
	const onUp = () => fan("onMouseUp");
	const onEnd = () => fan("onTouchEnd");
	return { register(handlers) {
		callbacks.add(handlers);
		instanceCount++;
		if (instanceCount === 1 && typeof window !== "undefined") {
			window.addEventListener("mousemove", onMove);
			window.addEventListener("touchmove", onTouch);
			window.addEventListener("mousedown", onDown, true);
			window.addEventListener("touchstart", onStart, true);
			window.addEventListener("mouseup", onUp, true);
			window.addEventListener("touchend", onEnd, true);
		}
		return () => {
			callbacks.delete(handlers);
			instanceCount--;
			if (instanceCount === 0 && typeof window !== "undefined") {
				window.removeEventListener("mousemove", onMove);
				window.removeEventListener("touchmove", onTouch);
				window.removeEventListener("mousedown", onDown, true);
				window.removeEventListener("touchstart", onStart, true);
				window.removeEventListener("mouseup", onUp, true);
				window.removeEventListener("touchend", onEnd, true);
			}
		};
	} };
})();
function shapeConfigUsesMouse(raw) {
	if (!raw) return false;
	let cfg = raw;
	if (typeof raw === "string") try {
		cfg = JSON.parse(raw);
	} catch {
		return false;
	}
	if (!cfg || typeof cfg !== "object") return false;
	for (const key in cfg) {
		const v = cfg[key];
		if (v && typeof v === "object" && v.type === "mouse") return true;
	}
	return false;
}
function shaderRendererGPU() {
	let rootCtx = null;
	let root = null;
	let context = null;
	let textureManager = null;
	let dispatcher = null;
	let canvasElement = null;
	let isInitialized = false;
	let isInitializing = false;
	let isRendererReady = false;
	let initializationAbortController = null;
	const nodes = /* @__PURE__ */ new Map();
	let rootId = null;
	const parentToChildren = /* @__PURE__ */ new Map();
	let pendingRegistrationQueue = [];
	let pendingNodeUpdates = [];
	const pipelineCache = createPipelineCache({
		maxSize: 4,
		dispose: (bc) => disposeComposition(bc)
	});
	const liveCompositions = /* @__PURE__ */ new Set();
	let boundComposition = null;
	let desiredHash = null;
	let structuralDirty = true;
	let currentWidth = 0;
	let currentHeight = 0;
	let logicalWidth = 0;
	let logicalHeight = 0;
	let pixelRatio = 1;
	let cachedMaxTextureDim = 8192;
	let lastRequestedWidth = 0;
	let lastRequestedHeight = 0;
	let hasInitialDimensions = false;
	const pxResWidth = () => logicalWidth || currentWidth || 1;
	const pxResHeight = () => logicalHeight || currentHeight || 1;
	let shouldAnimate = true;
	let isVisible = false;
	let forceFullFrameRate = false;
	let lastRenderTime = 0;
	let frameIntervalCap = 0;
	let globalElapsedTime = 0;
	let sharedTimeOrigin = null;
	let pendingRenderRAF = null;
	let frameLocked = false;
	let pointerX = -10;
	let pointerY = -10;
	let pointerActive = false;
	let pointerSeen = false;
	let cachedCanvasRect = null;
	let mouseListenersNeeded = false;
	let globalEventUnregister = null;
	let resizeObserver = null;
	let intersectionObserver = null;
	let unloadHandler = null;
	let windowResizeHandler = null;
	let pendingResize = null;
	let isResizeScheduled = false;
	let rendererColorSpace = "p3-linear";
	let rendererToneMapping = "linear";
	let enablePerformanceTracking = false;
	let setColorSpaceModeFn = null;
	let onReadyCallback = null;
	let hasEmittedReady = false;
	let firstRenderPending = false;
	let onDeviceLostCallback = null;
	let deviceLostFired = false;
	let failureReason = null;
	let onUnavailableCallback = null;
	let hasNotifiedUnavailable = false;
	let uncapturedErrorDevice = null;
	let uncapturedErrorHandler = null;
	let uncapturedValidationErrors = 0;
	let consecutiveRenderErrors = 0;
	const performanceTracker = new PerformanceTracker();
	const frameParams = {
		deltaTime: 0,
		pointer: {
			x: -10,
			y: -10,
			seen: false
		},
		pointerActive: false,
		dimensions: {
			width: 0,
			height: 0
		}
	};
	const frameLoop = createFrameLoop(() => renderFrame());
	const sanitizeId = (id) => id.replace(/[^a-zA-Z0-9_]/g, "_");
	const findChildNodes = (parentId) => {
		const childIds = parentToChildren.get(parentId);
		if (!childIds) return [];
		const out = [];
		for (const cid of childIds) {
			const info = nodes.get(cid);
			if (info) out.push(info);
		}
		return out;
	};
	const findNodeByCustomId = (customId) => {
		const sanitized = sanitizeId(customId);
		for (const [nodeId, info] of nodes) if (info.metadata.id === sanitized) return nodeId;
		return null;
	};
	const computeDimExtent = (node, origin) => {
		if (origin === "center") return void 0;
		const decl = node.boundingBoxDeclaration;
		if (!decl) return void 0;
		const snap = {};
		for (const [k, u] of Object.entries(node.uniforms)) snap[k] = u._rawDimensional !== void 0 ? u._rawDimensional : u.value;
		subscribeSdfBoundsReresolve(node, snap.shapeSdfUrl);
		return boxHalfExtentsUV(decl, snap, pxResWidth(), pxResHeight());
	};
	const sdfBoundsSubscribed = /* @__PURE__ */ new WeakMap();
	const subscribeSdfBoundsReresolve = (node, url) => {
		if (typeof url !== "string" || !url || isSdfContentBoundsScanned(url)) return;
		const seen = sdfBoundsSubscribed.get(node) ?? /* @__PURE__ */ new Set();
		sdfBoundsSubscribed.set(node, seen);
		if (seen.has(url)) return;
		seen.add(url);
		getSdfContentBounds(url, () => {
			seen.delete(url);
			if (nodes.get(node.id) !== node) return;
			reresolveDimensionalUniforms(node);
			if (anyLayoutGroups()) markLayoutDirty();
			requestRender();
		});
	};
	const reresolveDimensionalUniforms = (node) => {
		const decl = node.boundingBoxDeclaration;
		const extra = node.extraDimProps;
		const plan = buildDimensionalPlan(decl, extra);
		if (!plan) return;
		const origin = node.uniforms["origin"]?.value ?? "center";
		const he = computeDimExtent(node, origin);
		const names = [...plan.positionProps, ...plan.sizeProps.keys()];
		for (const name of names) {
			const u = node.uniforms[name];
			if (!u || u._rawDimensional === void 0) continue;
			const resolved = resolveDimensionalProp(decl, name, u._rawDimensional, origin, pxResWidth(), pxResHeight(), extra, he);
			u.value = resolved;
			writePropHandle(node, name, resolved);
		}
	};
	const resolveDynamicBound = (node, propName, bound) => {
		if (typeof bound === "number") return bound;
		const origin = node.uniforms["origin"]?.value ?? "center";
		const resolved = resolveDimensionalProp(node.boundingBoxDeclaration, propName, bound, origin, pxResWidth(), pxResHeight(), node.extraDimProps);
		return typeof resolved === "number" ? resolved : bound?.value ?? 0;
	};
	const aspectRatio = () => {
		const { width, height } = bufferSize();
		return width / Math.max(1, height);
	};
	const transformFieldValues = (node) => {
		const t = node.metadata.transform;
		return {
			offsetX: t?.offsetX ?? 0,
			offsetY: t?.offsetY ?? 0,
			rotation: t?.rotation ?? 0,
			scale: t?.scale ?? 1,
			anchorX: t?.anchorX ?? .5,
			anchorY: t?.anchorY ?? .5,
			aspectRatio: aspectRatio()
		};
	};
	const bboxFieldValues = (node) => {
		const bbox = node.metadata.boundingBox;
		if (!bbox) return null;
		const p = boundingBoxToUVParams(bbox, pxResWidth(), pxResHeight());
		return {
			centerX: p.centerX,
			centerY: p.centerY,
			halfWidth: p.halfWidthUV,
			halfHeight: p.halfHeightUV,
			cornerRadius: p.cornerRadiusUV,
			rotation: p.rotation,
			aspectRatio: aspectRatio()
		};
	};
	const bboxActive = (node) => !!node.metadata.boundingBox && !!node.boundingBoxDeclaration && !isIdentityBoundingBox(node.metadata.boundingBox);
	const computeNodeBoundsUV = (node) => {
		const decl = node.boundingBoxDeclaration;
		if (!decl || !node.uniforms) return null;
		if (!decl.computeBounds && !decl.propBindings) return null;
		const snap = {};
		for (const [k, u] of Object.entries(node.uniforms)) snap[k] = u._rawDimensional !== void 0 ? u._rawDimensional : u.value;
		const he = boxHalfExtentsUV(decl, snap, pxResWidth(), pxResHeight());
		if (he.hwx <= 0 && he.hhy <= 0) return null;
		let cx = .5;
		let cy = .5;
		const posProp = decl.propBindings?.x?.prop ?? decl.propBindings?.y?.prop;
		const posValue = posProp ? node.uniforms[posProp]?.value : void 0;
		if (posValue && typeof posValue.x === "number" && typeof posValue.y === "number") {
			cx = posValue.x;
			cy = posValue.y;
		}
		if (posProp) {
			const driver = node.metadata.maps?.[posProp];
			if (driver?.type === "mouse-position") {
				const s = node.mouseDriverState[posProp];
				if (s) {
					cx = s.liveX ?? (typeof driver.x === "number" ? driver.x : s.currentX);
					cy = s.liveY ?? (typeof driver.y === "number" ? driver.y : s.currentY);
				}
			}
		}
		return {
			centerX: cx,
			centerY: cy,
			halfWidth: he.hwx,
			halfHeight: he.hhy
		};
	};
	const childBoundsValues = (node) => {
		if (!node.definition.wantsBoundsParams) return null;
		const children = findChildNodes(node.id).sort((a, b) => a.metadata.renderOrder - b.metadata.renderOrder);
		for (const child of children) {
			if (child.metadata.visible === false) continue;
			if (bboxActive(child)) return null;
			const b = computeNodeBoundsUV(child);
			if (b) return b;
		}
		return null;
	};
	const updateChildBounds = () => {
		for (const node of nodes.values()) {
			if (!node.definition.wantsBoundsParams) continue;
			const b = childBoundsValues(node);
			if (!b) continue;
			writeSyntheticScalar(node, "_childBounds_centerX", b.centerX);
			writeSyntheticScalar(node, "_childBounds_centerY", b.centerY);
			writeSyntheticScalar(node, "_childBounds_halfWidth", b.halfWidth);
			writeSyntheticScalar(node, "_childBounds_halfHeight", b.halfHeight);
		}
	};
	let layoutDirty = false;
	const markLayoutDirty = () => {
		layoutDirty = true;
		requestRender();
	};
	const anyLayoutGroups = () => {
		for (const node of nodes.values()) {
			const m = node.metadata.flow?.mode;
			if (m === "column" || m === "row") return true;
		}
		return false;
	};
	const buildLayoutView = (node) => {
		const snap = {};
		for (const [k, u] of Object.entries(node.uniforms)) snap[k] = u._rawDimensional !== void 0 ? u._rawDimensional : u.value;
		const posProp = node.boundingBoxDeclaration?.propBindings?.x?.prop ?? node.boundingBoxDeclaration?.propBindings?.y?.prop;
		const posValue = posProp ? node.uniforms[posProp]?.value : void 0;
		const positionDriven = !!(posProp && (posValue?.type === "mouse-position" || node.metadata.maps?.[posProp]));
		return {
			id: node.id,
			componentName: node.componentName,
			visible: node.metadata.visible !== false,
			absolute: node.metadata.absolute === true,
			requiresChild: node.requiresChild,
			layout: node.metadata.flow,
			decl: node.boundingBoxDeclaration,
			props: snap,
			boundingBox: node.metadata.boundingBox,
			positionDriven,
			children: findChildNodes(node.id).sort((a, b) => a.metadata.renderOrder - b.metadata.renderOrder).map(buildLayoutView)
		};
	};
	const runLayoutPass = () => {
		if (!anyLayoutGroups()) {
			layoutDirty = false;
			return;
		}
		const cw = pxResWidth();
		const ch = pxResHeight();
		if (cw <= 1 || ch <= 1) return;
		layoutDirty = false;
		const roots = [];
		for (const node of nodes.values()) if (node.parentId === null) roots.push(buildLayoutView(node));
		const writes = computeTreeLayout(roots, cw, ch);
		for (const w of writes) {
			const node = nodes.get(w.id);
			if (!node) continue;
			if (w.kind === "position") {
				const u = node.uniforms[w.prop];
				if (!u) continue;
				const prev = u.value;
				const next = prev && typeof prev === "object" ? {
					...prev,
					x: w.value.x,
					y: w.value.y
				} : w.value;
				u.value = next;
				writePropHandle(node, w.prop, next);
			} else {
				const wasActive = bboxActive(node);
				node.metadata.boundingBox = {
					x: {
						value: w.xPx,
						unit: "px"
					},
					y: {
						value: w.yPx,
						unit: "px"
					},
					width: {
						value: w.widthPx,
						unit: "px"
					},
					height: {
						value: w.heightPx,
						unit: "px"
					},
					origin: "top-left",
					rotation: node.metadata.boundingBox?.rotation ?? 0,
					...node.metadata.boundingBox?.cornerRadius ? { cornerRadius: node.metadata.boundingBox.cornerRadius } : {},
					...node.metadata.boundingBox?.lockAspect !== void 0 ? { lockAspect: node.metadata.boundingBox.lockAspect } : {}
				};
				if (!wasActive && bboxActive(node)) markStructuralDirty();
				else {
					const bb = bboxFieldValues(node);
					if (bb) for (const [k, v] of Object.entries(bb)) writeSyntheticScalar(node, `_bbox_${k}`, v);
				}
			}
		}
	};
	const runLayoutPassIfDirty = () => {
		if (layoutDirty) runLayoutPass();
	};
	let unsubscribeNaturalSize = null;
	const onFontsLoadedLayout = () => {
		if (anyLayoutGroups()) markLayoutDirty();
	};
	const attachLayoutListeners = () => {
		if (!unsubscribeNaturalSize) unsubscribeNaturalSize = onNaturalSizeChange(() => {
			if (anyLayoutGroups()) markLayoutDirty();
		});
		if (typeof document !== "undefined" && document.fonts?.addEventListener) document.fonts.addEventListener("loadingdone", onFontsLoadedLayout);
	};
	attachLayoutListeners();
	const transformActive = (node) => needsTransformation(node.metadata.transform);
	const ensureMapValues = (node, prop, driver) => {
		const mv = node.mapValues[prop] ?? {
			inputMin: 0,
			inputMax: 1,
			outputMin: 0,
			outputMax: 1,
			curve: 0
		};
		if (driver.type === "map") {
			const c = driver;
			mv.inputMin = c.inputMin;
			mv.inputMax = c.inputMax;
			mv.outputMin = resolveDynamicBound(node, prop, c.outputMin);
			mv.outputMax = resolveDynamicBound(node, prop, c.outputMax);
			mv.curve = c.curve ?? 0;
		} else if (driver.type === "mouse") {
			const c = driver;
			mv.outputMin = resolveDynamicBound(node, prop, c.outputMin);
			mv.outputMax = resolveDynamicBound(node, prop, c.outputMax);
			mv.curve = c.curve ?? 0;
		} else if (driver.type === "auto-animate") {
			const c = driver;
			mv.outputMin = resolveDynamicBound(node, prop, c.outputMin);
			mv.outputMax = resolveDynamicBound(node, prop, c.outputMax);
		}
		node.mapValues[prop] = mv;
		return mv;
	};
	const ensureMouseState = (node, prop, driver) => {
		let s = node.mouseDriverState[prop];
		if (s) return s;
		if (driver.type === "mouse-position") {
			const trackX = driver.x === void 0 || driver.x === "mouse";
			const trackY = driver.y === void 0 || driver.y === "mouse";
			s = {
				currentX: trackX ? pointerX : driver.x,
				currentY: trackY ? pointerY : driver.y,
				velocityX: 0,
				velocityY: 0,
				pointerSnapped: pointerSeen
			};
		} else s = {
			currentX: Math.max(.001, driver.axis === "x" ? pointerX : pointerY),
			currentY: 0,
			velocityX: 0,
			velocityY: 0,
			pointerSnapped: pointerSeen
		};
		node.mouseDriverState[prop] = s;
		return s;
	};
	const ensureDriverState = (node) => {
		const maps = node.metadata.maps;
		if (!maps) return;
		for (const [prop, driver] of Object.entries(maps)) {
			if (!(prop in node.uniforms)) continue;
			if (driver.type === "map") ensureMapValues(node, prop, driver);
			else if (driver.type === "mouse-position") ensureMouseState(node, prop, driver);
			else if (driver.type === "mouse") {
				ensureMouseState(node, prop, driver);
				ensureMapValues(node, prop, driver);
			} else if (driver.type === "auto-animate") {
				if (!node.autoAnimateState[prop]) node.autoAnimateState[prop] = {};
				ensureMapValues(node, prop, driver);
			}
		}
	};
	const animatedTimeKey = (node) => {
		const at = node.definition.animatedTime;
		return (at ? node.uniforms[at.speed] : void 0) ?? node;
	};
	const extraAnimatedTimeKey = (node, speedProp) => node.uniforms[speedProp] ?? node;
	const buildFieldInits = (node) => {
		const inits = [];
		for (const [propName, u] of Object.entries(node.uniforms)) inits.push({
			name: propName,
			initial: u.value,
			transform: u.transform,
			cpu: u.cpu,
			schema: u.schema
		});
		inits.push({
			name: "_opacity",
			schema: f32,
			initial: node.metadata.opacity ?? 1
		});
		if (node.definition.animatedTime) inits.push({
			name: "_animTime",
			schema: f32,
			initial: getAnimatedTimeState(animatedTimeKey(node)).value
		});
		if (node.definition.extraAnimatedTimes) for (const [key, speedProp] of Object.entries(node.definition.extraAnimatedTimes)) inits.push({
			name: `_animTime_${key}`,
			schema: f32,
			initial: getAnimatedTimeState(extraAnimatedTimeKey(node, speedProp)).value
		});
		const extraFields = node.definition.extraFields;
		if (extraFields) for (const [name, f] of Object.entries(extraFields)) inits.push({
			name,
			schema: f.schema,
			initial: f.initial
		});
		if (transformActive(node)) {
			const tf = transformFieldValues(node);
			for (const [k, v] of Object.entries(tf)) inits.push({
				name: `_xform_${k}`,
				schema: f32,
				initial: v
			});
		}
		if (bboxActive(node)) {
			const bb = bboxFieldValues(node);
			if (bb) for (const [k, v] of Object.entries(bb)) inits.push({
				name: `_bbox_${k}`,
				schema: f32,
				initial: v
			});
		}
		if (node.definition.wantsBoundsParams) {
			const cb = childBoundsValues(node) ?? {
				centerX: .5,
				centerY: .5,
				halfWidth: .5,
				halfHeight: .5
			};
			for (const [k, v] of Object.entries(cb)) inits.push({
				name: `_childBounds_${k}`,
				schema: f32,
				initial: v
			});
		}
		const maps = node.metadata.maps;
		if (maps) {
			ensureDriverState(node);
			for (const [prop, driver] of Object.entries(maps)) {
				if (!(prop in node.uniforms)) continue;
				if (driver.type === "map") {
					const mv = node.mapValues[prop];
					inits.push({
						name: `_map_${prop}_inputMin`,
						schema: f32,
						initial: mv.inputMin
					});
					inits.push({
						name: `_map_${prop}_inputMax`,
						schema: f32,
						initial: mv.inputMax
					});
					inits.push({
						name: `_map_${prop}_outputMin`,
						schema: f32,
						initial: mv.outputMin
					});
					inits.push({
						name: `_map_${prop}_outputMax`,
						schema: f32,
						initial: mv.outputMax
					});
					inits.push({
						name: `_map_${prop}_curve`,
						schema: f32,
						initial: mv.curve
					});
				} else if (driver.type === "mouse-position") {
					const s = node.mouseDriverState[prop];
					inits.push({
						name: `_smoothed_${prop}`,
						schema: vec2f$1,
						initial: [s.currentX, 1 - s.currentY]
					});
				} else if (driver.type === "mouse") {
					const mv = node.mapValues[prop];
					inits.push({
						name: `_smoothed_${prop}`,
						schema: f32,
						initial: mv.outputMin
					});
				} else if (driver.type === "auto-animate") {
					const mv = node.mapValues[prop];
					inits.push({
						name: `_smoothed_${prop}`,
						schema: f32,
						initial: mv.outputMin
					});
				}
			}
		}
		return inits;
	};
	const writePropHandle = (node, propName, value) => {
		const h = node._liveHandles?.[propName];
		if (!h) return;
		setColorSpaceModeFn?.(rendererColorSpace);
		updateFieldValue(h, value);
	};
	const writeSyntheticScalar = (node, field, value) => {
		const h = node._liveHandles?.[field];
		if (h instanceof FieldHandle) h.value = value;
	};
	const writeExtraFieldValue = (node, field, value) => {
		const h = node._liveHandles?.[field];
		if (h instanceof FieldHandle) h.value = value;
		else if (h instanceof ArrayFieldHandle && Array.isArray(value)) h.array = value;
	};
	const nodeColorSpaceMode = (node) => {
		for (const u of Object.values(node.uniforms)) if (u.transform === transformColorSpace) {
			const raw = u.value;
			if (typeof raw === "string") return transformColorSpace(raw);
			if (typeof raw === "number") return raw;
		}
		return 0;
	};
	const repackColorStops = (node) => {
		if (!node.uniforms.convertedColorsArray) return;
		let stops = null;
		for (const u of Object.values(node.uniforms)) if (u.transform === colorStopsTransform) {
			stops = u.value ?? null;
			break;
		}
		const packed = packStops(stops, colorToRGBA);
		const converted = packConvertedStops(packed.colors, packed.stopCount, nodeColorSpaceMode(node));
		const write = (name, value) => {
			const u = node.uniforms[name];
			if (u) u.value = value;
			writePropHandle(node, name, value);
		};
		write("colorsArray", packed.colors);
		write("positionsArray", packed.positions);
		write("convertedColorsArray", converted);
		write("stopCount", packed.stopCount);
	};
	const repackListProp = (node, propName) => {
		const u = node.uniforms[propName];
		const spec = u?._listSpec;
		if (!u || !spec) return;
		const raw = u._rawList;
		const driven = /* @__PURE__ */ new Set();
		const items = resolveListItems(raw, spec, {
			color: colorToRGBA,
			drivenPosition: (index, field, driver) => {
				const key = listDriverPath(propName, index, field);
				driven.add(key);
				const st = ensureMouseState(node, key, driver);
				return st.liveX !== void 0 && st.liveY !== void 0 ? {
					x: st.liveX,
					y: 1 - st.liveY
				} : void 0;
			}
		});
		for (const key of Object.keys(node.mouseDriverState)) {
			const path = parseListDriverPath(key);
			if (path && path.prop === propName && !driven.has(key)) delete node.mouseDriverState[key];
		}
		u.value = items;
		writePropHandle(node, propName, items);
		const packed = packList(items, spec);
		const write = (name, value) => {
			const f = node.uniforms[name];
			if (f) f.value = value;
			writePropHandle(node, name, value);
		};
		for (const [field, lanes] of Object.entries(packed.fields)) write(listFieldName(propName, field), lanes);
		write(listCountName(propName), packed.count);
	};
	const driverForKey = (node, key) => {
		const top = node.metadata.maps?.[key];
		if (top) return top;
		const path = parseListDriverPath(key);
		if (!path) return void 0;
		const raw = node.uniforms[path.prop]?._rawList;
		const value = (Array.isArray(raw) ? raw[path.index] : void 0)?.[path.field];
		return isPositionDriver(value) ? value : void 0;
	};
	const listsUseMouse = (node) => Object.values(node.uniforms).some((u) => u._listSpec && listHasDrivers(u._rawList, u._listSpec));
	const writeNodeSynthetic = (node) => {
		writeSyntheticScalar(node, "_opacity", node.metadata.opacity ?? 1);
		if (node.definition.animatedTime) writeSyntheticScalar(node, "_animTime", getAnimatedTimeState(animatedTimeKey(node)).value);
		if (node.definition.extraAnimatedTimes) for (const [key, speedProp] of Object.entries(node.definition.extraAnimatedTimes)) writeSyntheticScalar(node, `_animTime_${key}`, getAnimatedTimeState(extraAnimatedTimeKey(node, speedProp)).value);
		if (transformActive(node)) for (const [k, v] of Object.entries(transformFieldValues(node))) writeSyntheticScalar(node, `_xform_${k}`, v);
		if (bboxActive(node)) {
			const bb = bboxFieldValues(node);
			if (bb) for (const [k, v] of Object.entries(bb)) writeSyntheticScalar(node, `_bbox_${k}`, v);
		}
		const maps = node.metadata.maps;
		if (maps) {
			for (const [prop, driver] of Object.entries(maps)) if (driver.type === "map" && node.mapValues[prop]) {
				const mv = node.mapValues[prop];
				writeSyntheticScalar(node, `_map_${prop}_inputMin`, mv.inputMin);
				writeSyntheticScalar(node, `_map_${prop}_inputMax`, mv.inputMax);
				writeSyntheticScalar(node, `_map_${prop}_outputMin`, mv.outputMin);
				writeSyntheticScalar(node, `_map_${prop}_outputMax`, mv.outputMax);
				writeSyntheticScalar(node, `_map_${prop}_curve`, mv.curve);
			}
		}
	};
	const propHandleView = (node, handles) => {
		const out = {};
		for (const propName of Object.keys(node.uniforms)) {
			const h = handles[propName];
			if (h) out[propName] = h;
		}
		return out;
	};
	const sotHandleView = (node) => {
		const out = {};
		for (const [propName, u] of Object.entries(node.uniforms)) out[propName] = {
			accessorPath: "",
			cpu: !!u.cpu,
			value: u.value
		};
		return out;
	};
	const makeRegistryNode = (node, handles) => ({
		id: node.id,
		customId: node.metadata.id,
		componentName: node.componentName,
		parentId: node.parentId,
		definition: node.definition,
		metadata: node.metadata,
		handles,
		domCanvas: node.domCanvas
	});
	const buildRegistryView = (store, handlesById) => {
		const cache = /* @__PURE__ */ new Map();
		const build = (node) => {
			return makeRegistryNode(node, store && handlesById ? propHandleView(node, handlesById.get(node.id) ?? {}) : sotHandleView(node));
		};
		const getNode = (id) => {
			const hit = cache.get(id);
			if (hit) return hit;
			const node = nodes.get(id);
			if (!node) return void 0;
			const rn = build(node);
			cache.set(id, rn);
			return rn;
		};
		return {
			rootId,
			getNode,
			getChildren: (pid) => findChildNodes(pid).map((n) => getNode(n.id)),
			resolveCustomId: (cid) => findNodeByCustomId(cid),
			store: {
				gpuAccessor: (h, layoutVar) => store ? store.gpuAccessor(h, layoutVar) : "",
				layout: store?.layout
			}
		};
	};
	const composeOptions = () => ({
		toneMapping: rendererToneMapping,
		flipY: false,
		premultiplyAlpha: true,
		canvas: canvasElement ?? void 0,
		dimensions: bufferSize(),
		gpu: root ? {
			device: root.device,
			root
		} : void 0,
		writeExtraField: (nodeId, name, value) => {
			const node = nodes.get(nodeId);
			if (node) writeExtraFieldValue(node, name, value);
		},
		createMediaTexture: (opts) => {
			if (!textureManager) throw new Error("[gpu] createMediaTexture before init");
			return textureManager.createMediaTexture(opts);
		},
		createDataTexture: (opts) => {
			if (!textureManager) throw new Error("[gpu] createDataTexture before init");
			return textureManager.createDataTexture(opts);
		},
		resolveMapWindow: (nodeId, prop) => {
			const node = nodes.get(nodeId);
			const driver = node?.metadata.maps?.[prop];
			if (!node || !driver || driver.type !== "map") return void 0;
			const mv = ensureMapValues(node, prop, driver);
			return {
				inputMin: mv.inputMin,
				inputMax: mv.inputMax,
				outputMin: mv.outputMin,
				outputMax: mv.outputMax,
				curve: mv.curve
			};
		},
		resolveDriverCpuValue: (nodeId, prop) => {
			const node = nodes.get(nodeId);
			if (!node) return void 0;
			if (prop === "_animTime" || prop.startsWith("_animTime_")) {
				const h = node._liveHandles?.[prop];
				return h instanceof FieldHandle ? h.value : void 0;
			}
			const driver = node.metadata.maps?.[prop];
			if (!driver) return void 0;
			if (driver.type === "mouse-position") {
				const s = node.mouseDriverState[prop];
				if (!s || s.liveX === void 0 || s.liveY === void 0) return void 0;
				return {
					x: s.liveX,
					y: 1 - s.liveY
				};
			}
			if (driver.type === "mouse" || driver.type === "auto-animate") {
				const h = node._liveHandles?.[`_smoothed_${prop}`];
				return h instanceof FieldHandle ? h.value : void 0;
			}
		}
	});
	const extraHashInputs = () => {
		const parts = [];
		for (const [id, node] of nodes) {
			const m = node.metadata;
			parts.push(`aopac:${id}:${(m.opacity ?? 1) < 1}`);
			const sc = node.uniforms.stopCount?.value;
			if (typeof sc === "number") parts.push(`stops:${id}:${sc > 1 ? sc : 0}`);
			for (const [prop, u] of Object.entries(node.uniforms)) {
				if (!u.compileTimeWhen) continue;
				const v = u._lastCompiledValue;
				if (typeof v !== "number" && typeof v !== "boolean") continue;
				parts.push(`ctw:${id}:${prop}:${String(v)}`);
			}
			if (m.maps) {
				const fp = Object.entries(m.maps).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([p, dr]) => {
					const cfg = dr;
					return `${p}=${cfg.type}:${cfg.source ?? ""}:${cfg.channel ?? ""}:${cfg.axis ?? ""}`;
				}).join(",");
				parts.push(`mapfp:${id}:${fp}`);
			}
		}
		return parts;
	};
	const computeStructuralHash = () => {
		if (!rootId) return null;
		return structuralHash([...collectStructuralHashInputs(buildRegistryView(null, null), composeOptions()), ...extraHashInputs()]);
	};
	const bufferSize = () => ({
		width: Math.max(1, Math.round(currentWidth * pixelRatio)),
		height: Math.max(1, Math.round(currentHeight * pixelRatio))
	});
	const mediaResourcesFromIr = (ir) => {
		const out = {};
		for (const t of ir.textures) if (t.kind !== "rtt" && t.resource) out[t.key] = t.resource;
		return out;
	};
	const buildComposition = (hash) => {
		if (!root || !textureManager || !dispatcher) throw new Error("[gpu] buildComposition before init");
		const store = createUniformStore(root, { systemSchema: SystemUniforms });
		const handlesById = /* @__PURE__ */ new Map();
		for (const [id, node] of nodes) handlesById.set(id, store.defineNode(id, buildFieldInits(node)));
		store.defineSystem();
		store.finalize();
		const ir = composeNodeTree(buildRegistryView(store, handlesById), composeOptions());
		const pm = createPassManager(root, {
			textureManager,
			dispatcher
		});
		const bc = {
			hash,
			store,
			ir,
			passManager: pm,
			handlesById,
			fresh: true,
			validationFrames: 3
		};
		const device = ir.usesCustomWgsl ? root.device : void 0;
		if (device) device.pushErrorScope("validation");
		try {
			pm.setComposition(ir, store.bindGroup, bufferSize(), mediaResourcesFromIr(ir));
		} finally {
			if (device) device.popErrorScope().then((error) => {
				if (error && !bc.broken) markBroken(bc, error.message);
			}).catch(() => {});
		}
		liveCompositions.add(bc);
		return bc;
	};
	const markBroken = (bc, message) => {
		bc.broken = message;
		authorError("[gpu] a shader in this composition failed GPU validation — it will not draw until it changes. If you are writing a wgsl`…` body, the message below points at the line:\n" + message);
	};
	const disposeComposition = (bc) => {
		try {
			for (const cb of bc.ir.onCleanup) cb();
		} catch (e) {
			debugError("[gpu] composition onCleanup error:", e);
		}
		try {
			bc.passManager.dispose();
		} catch (e) {
			debugError("[gpu] passManager dispose error:", e);
		}
		try {
			bc.store.destroy();
		} catch (e) {
			debugError("[gpu] store destroy error:", e);
		}
		liveCompositions.delete(bc);
		if (boundComposition === bc) boundComposition = null;
	};
	const bindComposition = (bc) => {
		let repointed = false;
		for (const [id, handles] of bc.handlesById) {
			const node = nodes.get(id);
			if (node && node._liveHandles !== handles) {
				node._liveHandles = handles;
				repointed = true;
			}
		}
		if (bc === boundComposition && !repointed) return;
		if (!bc.fresh) {
			for (const node of nodes.values()) {
				for (const [propName, u] of Object.entries(node.uniforms)) writePropHandle(node, propName, u.value);
				ensureDriverState(node);
				writeNodeSynthetic(node);
			}
			bc.store.writeAll();
		}
		bc.fresh = false;
		boundComposition = bc;
		if (!hasEmittedReady && bc.ir.composedNodeIds.size > 0) firstRenderPending = true;
	};
	const markStructuralDirty = () => {
		structuralDirty = true;
		if (anyLayoutGroups()) layoutDirty = true;
		requestRender();
	};
	const requestRender = () => {
		if (failureReason || frameLocked) return;
		if (frameLoop.running) return;
		if (pendingRenderRAF !== null) return;
		if (typeof requestAnimationFrame !== "function") return;
		pendingRenderRAF = requestAnimationFrame(() => {
			pendingRenderRAF = null;
			renderFrame();
		});
	};
	const registerNode = (id, fragmentNodeFunc, parentId, metadata, uniforms = {}, componentDefinition, domCanvas) => {
		if (failureReason) return;
		if (fragmentNodeFunc === null) {
			const queueIndex = pendingRegistrationQueue.findIndex((item) => item.id === id);
			if (queueIndex !== -1) {
				pendingRegistrationQueue.splice(queueIndex, 1);
				pendingNodeUpdates = pendingNodeUpdates.filter((u) => u.id !== id);
				return;
			}
			removeNode(id);
			return;
		}
		if (!componentDefinition) {
			console.error(`[gpu] registerNode("${id}") requires a componentDefinition`);
			return;
		}
		if (!isRendererReady) {
			const existing = pendingRegistrationQueue.findIndex((item) => item.id === id);
			const entry = {
				id,
				definition: componentDefinition,
				parentId,
				metadata,
				uniforms,
				domCanvas
			};
			if (existing !== -1) {
				pendingRegistrationQueue[existing] = entry;
				pendingNodeUpdates = pendingNodeUpdates.filter((u) => u.id !== id);
			} else pendingRegistrationQueue.push(entry);
			return;
		}
		let defaultRenderOrder = 0;
		if (parentId !== null) {
			const siblings = parentToChildren.get(parentId);
			if (siblings && siblings.size > 0) {
				let maxOrder = 0;
				for (const siblingId of siblings) {
					const sibling = nodes.get(siblingId);
					if (sibling && sibling.metadata.renderOrder > maxOrder) maxOrder = sibling.metadata.renderOrder;
				}
				defaultRenderOrder = maxOrder + 1;
			}
		}
		const nodeMetadata = {
			blendMode: metadata?.blendMode || "normal",
			opacity: metadata?.opacity,
			visible: metadata?.visible === false ? false : true,
			id: metadata?.id ? sanitizeId(metadata.id) : metadata?.id,
			mask: metadata?.mask,
			maps: metadata?.maps,
			renderOrder: metadata?.renderOrder !== void 0 ? metadata.renderOrder : defaultRenderOrder,
			transform: metadata?.transform,
			boundingBox: metadata?.boundingBox,
			flow: metadata?.flow,
			absolute: metadata?.absolute
		};
		const decl = componentDefinition.boundingBoxDeclaration;
		const extraDimProps = componentDefinition.props ? buildExtraSizeProps(componentDefinition.props) : void 0;
		const node = {
			id,
			componentName: componentDefinition.name || "Unknown",
			parentId,
			definition: componentDefinition,
			metadata: nodeMetadata,
			uniforms,
			domCanvas,
			requiresRTT: !!componentDefinition.requiresRTT,
			requiresChild: !!componentDefinition.requiresChild,
			blendWithChildren: !!componentDefinition.blendWithChildren,
			acceptsUVContext: !!componentDefinition.acceptsUVContext,
			providesUVContextViaCompute: !!componentDefinition.providesUVContextViaCompute,
			usesPointer: !!componentDefinition.usesPointer,
			hasUvRemap: !!componentDefinition.uvRemap,
			boundingBoxDeclaration: decl,
			wantsBoundsParams: !!componentDefinition.wantsBoundsParams,
			extraDimProps,
			mouseDriverState: {},
			autoAnimateState: {},
			mapValues: {},
			_liveHandles: null
		};
		if (decl || extraDimProps) {
			const plan = buildDimensionalPlan(decl, extraDimProps);
			if (plan) {
				const origin = uniforms["origin"]?.value ?? "center";
				const he = computeDimExtent(node, origin);
				for (const name of [...plan.positionProps, ...plan.sizeProps.keys()]) {
					const u = uniforms[name];
					if (!u) continue;
					if (u._rawDimensional === void 0) u._rawDimensional = u.value;
					u.value = resolveDimensionalProp(decl, name, u._rawDimensional, origin, pxResWidth(), pxResHeight(), extraDimProps, he);
				}
			}
		}
		for (const u of Object.values(uniforms)) if (u.compileTimeWhen && u._lastCompiledValue === void 0) u._lastCompiledValue = u.value;
		nodes.set(id, node);
		if (parentId === null) rootId = id;
		const childrenSet = parentToChildren.get(parentId) || /* @__PURE__ */ new Set();
		childrenSet.add(id);
		parentToChildren.set(parentId, childrenSet);
		updateMouseListenerRegistration();
		markStructuralDirty();
		if (anyLayoutGroups()) markLayoutDirty();
	};
	const deferUntilRegistered = (update) => {
		if (pendingRegistrationQueue.some((q) => q.id === update.id)) pendingNodeUpdates.push(update);
	};
	const updateUniformValue = (nodeId, uniformName, value) => {
		const node = nodes.get(nodeId);
		if (!node) {
			deferUntilRegistered({
				id: nodeId,
				kind: "uniform",
				name: uniformName,
				value
			});
			return;
		}
		const u = node.uniforms[uniformName];
		if (!u) return;
		const decl = node.boundingBoxDeclaration;
		const extra = node.extraDimProps;
		if ((decl || extra) && isDimensionalProp(decl, uniformName, extra)) {
			u._rawDimensional = value;
			const origin = node.uniforms["origin"]?.value ?? "center";
			const he = computeDimExtent(node, origin);
			value = resolveDimensionalProp(decl, uniformName, value, origin, pxResWidth(), pxResHeight(), extra, he);
		}
		if (u.transform === listPropTransform) {
			u._rawList = value;
			repackListProp(node, uniformName);
			updateMouseListenerRegistration();
			requestRender();
			return;
		}
		u.value = value;
		writePropHandle(node, uniformName, value);
		if (u.transform === colorStopsTransform || u.transform === transformColorSpace) repackColorStops(node);
		if (uniformName === "origin" && decl) reresolveDimensionalUniforms(node);
		else if (decl) {
			if ((node.uniforms["origin"]?.value ?? "center") !== "center") {
				const plan = buildDimensionalPlan(decl, extra);
				const affectsExtent = !!decl.computeBounds || (plan?.sizeProps.has(uniformName) ?? false);
				const isPositionProp = plan?.positionProps.has(uniformName) ?? false;
				if (affectsExtent && !isPositionProp) reresolveDimensionalUniforms(node);
			}
		}
		if (uniformName === "shape") updateMouseListenerRegistration();
		if (anyLayoutGroups()) markLayoutDirty();
		if (u.compileTime) {
			markStructuralDirty();
			return;
		}
		if (u.compileTimeWhen) {
			const prev = u._lastCompiledValue;
			if (u.compileTimeWhen(prev, value)) {
				u._lastCompiledValue = value;
				markStructuralDirty();
				return;
			}
		}
		requestRender();
	};
	const updateNodeMetadata = (nodeId, metadata) => {
		const node = nodes.get(nodeId);
		if (!node) {
			deferUntilRegistered({
				id: nodeId,
				kind: "metadata",
				metadata
			});
			return;
		}
		let needsRecompose = false;
		if (metadata.blendMode !== void 0 && node.metadata.blendMode !== metadata.blendMode) {
			node.metadata.blendMode = metadata.blendMode;
			needsRecompose = true;
		}
		if (metadata.opacity !== void 0 && node.metadata.opacity !== metadata.opacity) {
			const oldOpacity = node.metadata.opacity ?? 1;
			const newOpacity = metadata.opacity;
			node.metadata.opacity = newOpacity;
			writeSyntheticScalar(node, "_opacity", newOpacity);
			if (oldOpacity >= 1 !== newOpacity >= 1) needsRecompose = true;
		}
		if (metadata.renderOrder !== void 0 && node.metadata.renderOrder !== metadata.renderOrder) {
			node.metadata.renderOrder = metadata.renderOrder;
			needsRecompose = true;
		}
		if ("visible" in metadata) {
			const newVisible = metadata.visible === false ? false : true;
			if (node.metadata.visible !== newVisible) {
				node.metadata.visible = newVisible;
				needsRecompose = true;
			}
		}
		if (metadata.id !== void 0) {
			const sanitized = sanitizeId(metadata.id);
			if (node.metadata.id !== sanitized) {
				node.metadata.id = sanitized;
				needsRecompose = true;
			}
		}
		if ("mask" in metadata && (node.metadata.mask?.source !== metadata.mask?.source || node.metadata.mask?.type !== metadata.mask?.type)) {
			node.metadata.mask = metadata.mask;
			needsRecompose = true;
		}
		if ("maps" in metadata && JSON.stringify(node.metadata.maps) !== JSON.stringify(metadata.maps)) {
			const oldMaps = node.metadata.maps;
			const newMaps = metadata.maps;
			node.metadata.maps = newMaps;
			updateMouseListenerRegistration();
			if ((() => {
				if (!oldMaps || !newMaps) return true;
				const oldKeys = Object.keys(oldMaps);
				const newKeys = Object.keys(newMaps);
				if (oldKeys.length !== newKeys.length || !oldKeys.every((k) => k in newMaps)) return true;
				for (const key of newKeys) {
					const oldCfg = oldMaps[key];
					const newCfg = newMaps[key];
					if (oldCfg.type !== newCfg.type) return true;
					if (newCfg.type === "map") {
						const oc = oldCfg;
						const nc = newCfg;
						if (oc.source !== nc.source || oc.channel !== nc.channel) return true;
					} else if (newCfg.type === "mouse") {
						if (oldCfg.axis !== newCfg.axis) return true;
					}
				}
				return false;
			})()) {
				if (oldMaps && newMaps) {
					for (const key of Object.keys(oldMaps)) if (key in newMaps && oldMaps[key].type !== newMaps[key].type) {
						delete node.mouseDriverState[key];
						delete node.autoAnimateState[key];
						delete node.mapValues[key];
					}
				}
				needsRecompose = true;
			} else if (newMaps) for (const [key, driver] of Object.entries(newMaps)) {
				const mv = ensureMapValues(node, key, driver);
				if (driver.type === "map") {
					writeSyntheticScalar(node, `_map_${key}_inputMin`, mv.inputMin);
					writeSyntheticScalar(node, `_map_${key}_inputMax`, mv.inputMax);
					writeSyntheticScalar(node, `_map_${key}_outputMin`, mv.outputMin);
					writeSyntheticScalar(node, `_map_${key}_outputMax`, mv.outputMax);
					writeSyntheticScalar(node, `_map_${key}_curve`, mv.curve);
				}
			}
		}
		if (metadata.transform !== void 0) {
			const prev = node.metadata.transform;
			const merged = {
				offsetX: metadata.transform.offsetX ?? prev?.offsetX ?? 0,
				offsetY: metadata.transform.offsetY ?? prev?.offsetY ?? 0,
				rotation: metadata.transform.rotation ?? prev?.rotation ?? 0,
				scale: metadata.transform.scale ?? prev?.scale ?? 1,
				anchorX: metadata.transform.anchorX ?? prev?.anchorX ?? .5,
				anchorY: metadata.transform.anchorY ?? prev?.anchorY ?? .5,
				edges: metadata.transform.edges ?? prev?.edges ?? "transparent"
			};
			const wasActive = transformActive(node);
			node.metadata.transform = merged;
			const nowActive = needsTransformation(merged);
			if (wasActive !== nowActive || prev && prev.edges !== merged.edges) needsRecompose = true;
			else if (nowActive) for (const [k, v] of Object.entries(transformFieldValues(node))) writeSyntheticScalar(node, `_xform_${k}`, v);
		}
		if (metadata.boundingBox !== void 0 && metadata.boundingBox !== null && node.boundingBoxDeclaration !== void 0) {
			const wasActive = bboxActive(node);
			node.metadata.boundingBox = metadata.boundingBox;
			const nowActive = bboxActive(node);
			if (wasActive !== nowActive) needsRecompose = true;
			else if (nowActive) {
				const bb = bboxFieldValues(node);
				if (bb) for (const [k, v] of Object.entries(bb)) writeSyntheticScalar(node, `_bbox_${k}`, v);
			}
		} else if (metadata.boundingBox === null) {
			if (node.metadata.boundingBox !== void 0) {
				node.metadata.boundingBox = void 0;
				needsRecompose = true;
			}
		}
		if ("flow" in metadata && JSON.stringify(node.metadata.flow) !== JSON.stringify(metadata.flow)) {
			const wasActive = node.metadata.flow?.mode === "column" || node.metadata.flow?.mode === "row";
			node.metadata.flow = metadata.flow ?? void 0;
			const nowActive = node.metadata.flow?.mode === "column" || node.metadata.flow?.mode === "row";
			if (wasActive && !nowActive) {
				const restore = (id) => {
					for (const child of findChildNodes(id)) {
						reresolveDimensionalUniforms(child);
						restore(child.id);
					}
				};
				restore(nodeId);
			}
			markLayoutDirty();
		}
		if ("absolute" in metadata && node.metadata.absolute !== metadata.absolute) {
			node.metadata.absolute = metadata.absolute;
			if (metadata.absolute) reresolveDimensionalUniforms(node);
			markLayoutDirty();
		}
		if (anyLayoutGroups()) markLayoutDirty();
		if (needsRecompose) markStructuralDirty();
		else requestRender();
	};
	const removeNode = (id) => {
		const node = nodes.get(id);
		if (!node) return;
		for (const child of findChildNodes(id)) removeNode(child.id);
		if (rootId === id) rootId = null;
		const siblingSet = parentToChildren.get(node.parentId);
		if (siblingSet) {
			siblingSet.delete(id);
			if (siblingSet.size === 0) parentToChildren.delete(node.parentId);
		}
		nodes.delete(id);
		updateMouseListenerRegistration();
		markStructuralDirty();
		if (anyLayoutGroups()) markLayoutDirty();
	};
	const updateMouseDrivers = () => {
		const deltaTime = frameParams.deltaTime;
		globalElapsedTime = frameLocked ? globalElapsedTime + deltaTime : deriveElapsedTime(sharedTimeOrigin, globalElapsedTime, deltaTime, performance.now());
		for (const node of nodes.values()) {
			const maps = node.metadata.maps ?? {};
			for (const propName in node.mouseDriverState) {
				const state = node.mouseDriverState[propName];
				const driver = driverForKey(node, propName);
				if (!driver) continue;
				if (driver.type === "mouse-position") {
					const smoothing = driver.smoothing ?? 0;
					const momentum = driver.momentum ?? 0;
					const trackX = driver.x === void 0 || driver.x === "mouse";
					const trackY = driver.y === void 0 || driver.y === "mouse";
					if (!state.pointerSnapped && pointerSeen) {
						state.pointerSnapped = true;
						if (trackX) {
							state.currentX = pointerX;
							state.velocityX = 0;
						}
						if (trackY) {
							state.currentY = pointerY;
							state.velocityY = 0;
						}
					}
					let finalX;
					let finalY;
					if (trackX) {
						const [nx, vx] = applySpring(state.currentX, state.velocityX, pointerX, smoothing, momentum, deltaTime);
						state.currentX = nx;
						state.velocityX = vx;
						finalX = nx;
					} else {
						finalX = driver.x;
						state.velocityX = 0;
					}
					if (trackY) {
						const [ny, vy] = applySpring(state.currentY, state.velocityY, pointerY, smoothing, momentum, deltaTime);
						state.currentY = ny;
						state.velocityY = vy;
						finalY = ny;
					} else {
						finalY = driver.y;
						state.velocityY = 0;
					}
					const reach = driver.reach ?? 1;
					const originX = driver.originX ?? .5;
					const originY = driver.originY ?? .5;
					const xSign = driver.invertX ? -1 : 1;
					const ySign = driver.invertY ? -1 : 1;
					const scaledX = originX + xSign * (finalX - originX) * reach;
					const scaledY = originY + ySign * (finalY - originY) * reach;
					let localX = scaledX;
					let localY = scaledY;
					const bb = node.metadata.boundingBox;
					if (bb && node.boundingBoxDeclaration?.supportsResizeFit && bboxActive(node)) {
						const p = boundingBoxToUVParams(bb, pxResWidth(), pxResHeight());
						const bl = screenUVToBoxLocal$1(scaledX, scaledY, {
							centerX: p.centerX,
							centerY: p.centerY,
							halfWidthUV: p.halfWidthUV,
							halfHeightUV: p.halfHeightUV,
							rotationDeg: p.rotation,
							aspectRatio: aspectRatio()
						});
						localX = bl.x;
						localY = bl.y;
					}
					state.liveX = localX;
					state.liveY = localY;
					const h = node._liveHandles?.[`_smoothed_${propName}`];
					if (h instanceof FieldHandle) h.setComponents(localX, 1 - localY);
				} else if (driver.type === "mouse") {
					const smoothing = driver.smoothing ?? 0;
					const momentum = driver.momentum ?? 0;
					const target = driver.axis === "x" ? pointerX : pointerY;
					if (!state.pointerSnapped && pointerSeen) {
						state.pointerSnapped = true;
						state.currentX = Math.max(.001, target);
						state.velocityX = 0;
					}
					const [nx, vx] = applySpring(state.currentX, state.velocityX, target, smoothing, momentum, deltaTime);
					state.currentX = nx;
					state.velocityX = vx;
					const smoothed01 = Math.max(.001, nx);
					const mv = node.mapValues[propName];
					const resolved = mv ? mv.outputMin + (mv.outputMax - mv.outputMin) * Math.pow(smoothed01, Math.pow(2, -mv.curve * 2)) : smoothed01;
					state.liveScalar = resolved;
					writeSyntheticScalar(node, `_smoothed_${propName}`, resolved);
				}
			}
			for (const [prop, u] of Object.entries(node.uniforms)) if (u._listSpec && listHasDrivers(u._rawList, u._listSpec)) repackListProp(node, prop);
			for (const propName in node.autoAnimateState) {
				const driver = maps[propName];
				if (!driver || driver.type !== "auto-animate") continue;
				const globalT = globalElapsedTime * (driver.speed ?? 1) * .2;
				let phase;
				const easing = driver.easing ?? driver.waveform ?? "sine";
				if (driver.mode === "loop") phase = applyEasing((globalT % 1 + 1) % 1, easing);
				else {
					const t01 = (globalT % 1 + 1) % 1;
					phase = applyEasing(t01 < .5 ? t01 * 2 : (1 - t01) * 2, easing);
				}
				const mv = node.mapValues[propName];
				const outputMin = mv ? mv.outputMin : resolveDynamicBound(node, propName, driver.outputMin);
				const outputMax = mv ? mv.outputMax : resolveDynamicBound(node, propName, driver.outputMax);
				const newValue = outputMin + phase * (outputMax - outputMin);
				const u = node.uniforms[propName];
				if (u?.compileTimeWhen) {
					const prev = u._lastCompiledValue;
					if (u.compileTimeWhen(prev, newValue)) {
						u._lastCompiledValue = newValue;
						markStructuralDirty();
					}
				}
				writeSyntheticScalar(node, `_smoothed_${propName}`, newValue);
			}
		}
	};
	const updateAnimatedTime = () => {
		const dt = frameParams.deltaTime;
		for (const node of nodes.values()) {
			const at = node.definition.animatedTime;
			if (at) {
				const speedU = node.uniforms[at.speed];
				const speed = speedU ? Number(speedU.value) || 0 : 0;
				const state = getAnimatedTimeState(animatedTimeKey(node));
				state.advance(dt, speed);
				writeSyntheticScalar(node, "_animTime", state.value);
			}
			const extras = node.definition.extraAnimatedTimes;
			if (extras) for (const [key, speedProp] of Object.entries(extras)) {
				const su = node.uniforms[speedProp];
				const sp = su ? Number(su.value) || 0 : 0;
				const st = getAnimatedTimeState(extraAnimatedTimeKey(node, speedProp));
				st.advance(dt, sp);
				writeSyntheticScalar(node, `_animTime_${key}`, st.value);
			}
		}
	};
	const clampToTextureCap$1 = (w, h) => clampToTextureCap(w, h, {
		maxTextureDim: cachedMaxTextureDim,
		pixelRatio,
		viewportWidth: typeof window !== "undefined" ? window.innerWidth : w,
		viewportHeight: typeof window !== "undefined" ? window.innerHeight : h
	});
	const basePixelRatio = () => {
		const deviceRatio = (typeof window !== "undefined" ? window.devicePixelRatio : 1) || 1;
		return Math.min(deviceRatio, maxPixelRatioForDevice());
	};
	const applyRendererDimensions = (width, height, rawWidth, rawHeight) => {
		if (hasInitialDimensions && width === currentWidth && height === currentHeight && rawWidth === logicalWidth && rawHeight === logicalHeight) {
			const { width: curBufW, height: curBufH } = bufferSize();
			if (!canvasElement || canvasElement.width === curBufW && canvasElement.height === curBufH) return;
		}
		currentWidth = width;
		currentHeight = height;
		logicalWidth = rawWidth;
		logicalHeight = rawHeight;
		setShaderCanvasDimensions(rawWidth, rawHeight);
		if (!hasInitialDimensions) {
			hasInitialDimensions = true;
			if (nodes.size > 0) markStructuralDirty();
		}
		const { width: bufW, height: bufH } = bufferSize();
		if (canvasElement) {
			canvasElement.width = bufW;
			canvasElement.height = bufH;
		}
		let anyTextureRecreated = false;
		for (const c of liveCompositions) if (c.passManager.resize(bufW, bufH)) anyTextureRecreated = true;
		for (const c of liveCompositions) for (const cb of c.ir.onResize) try {
			cb({
				width: bufW,
				height: bufH
			});
		} catch (e) {
			debugError("[gpu] composition onResize error:", e);
		}
		for (const node of nodes.values()) {
			if (transformActive(node)) writeSyntheticScalar(node, "_xform_aspectRatio", aspectRatio());
			if (bboxActive(node)) {
				const bb = bboxFieldValues(node);
				if (bb) for (const [k, v] of Object.entries(bb)) writeSyntheticScalar(node, `_bbox_${k}`, v);
			}
			reresolveDimensionalUniforms(node);
		}
		runLayoutPass();
		if (anyTextureRecreated) for (const c of liveCompositions) {
			writeSystemUniforms(c.store);
			c.store.flush();
			c.passManager.repaintRtt();
		}
		if (frameLocked) return;
		lastRenderTime = 0;
		renderFrame();
	};
	const updateRendererDimensions = (width, height) => {
		if (width <= 0 || height <= 0) return;
		if (width === lastRequestedWidth && height === lastRequestedHeight) return;
		lastRequestedWidth = width;
		lastRequestedHeight = height;
		pendingResize = {
			width,
			height
		};
		if (!isResizeScheduled && typeof requestAnimationFrame === "function") {
			isResizeScheduled = true;
			requestAnimationFrame(() => {
				isResizeScheduled = false;
				if (!pendingResize) return;
				const raw = pendingResize;
				pendingResize = null;
				const { width: cw, height: ch } = clampToTextureCap$1(raw.width, raw.height);
				applyRendererDimensions(cw, ch, raw.width, raw.height);
			});
		}
	};
	const handleResize = (entries) => {
		const entry = entries[0];
		if (!entry) return;
		let width, height;
		if (entry.contentBoxSize) {
			const boxSize = Array.isArray(entry.contentBoxSize) ? entry.contentBoxSize[0] : entry.contentBoxSize;
			if (boxSize) {
				width = Math.round(boxSize.inlineSize);
				height = Math.round(boxSize.blockSize);
			} else {
				width = Math.round(entry.contentRect.width);
				height = Math.round(entry.contentRect.height);
			}
		} else {
			width = Math.round(entry.contentRect.width);
			height = Math.round(entry.contentRect.height);
		}
		if (width === 0 || height === 0) return;
		updateRendererDimensions(width, height);
	};
	const handleVisibilityChange = (entries) => {
		const entry = entries[0];
		if (!entry) return;
		isVisible = entry.isIntersecting;
	};
	const canRender = () => isInitialized && !!root && !!context && currentWidth > 0 && currentHeight > 0;
	const writeSystemUniforms = (store) => {
		const sys = store.systemHandles;
		const { width, height } = bufferSize();
		const bpr = basePixelRatio();
		const set = (name, value) => {
			const h = sys[name];
			if (h instanceof FieldHandle) h.value = value;
		};
		set("time", globalElapsedTime);
		set("viewportSize", [width, height]);
		set("logicalViewportSize", [pxResWidth() * bpr, pxResHeight() * bpr]);
		set("aspect", width / Math.max(1, height));
		set("pointer", [pointerX, pointerY]);
		set("pointerActive", pointerActive ? 1 : 0);
	};
	const runCallbacks = (callbacks) => {
		for (const cb of callbacks) try {
			cb(frameParams);
		} catch (e) {
			debugError("[gpu] render callback error:", e);
		}
	};
	const renderFrameInternal = (overrideDeltaTime) => {
		if (failureReason || !canRender()) return;
		const now = performance.now();
		let delta;
		if (overrideDeltaTime !== void 0) delta = overrideDeltaTime;
		else {
			const gate = frameGate(now, {
				lastRenderTime,
				isVisible,
				forceFullFrameRate,
				minInterval: frameIntervalCap
			});
			if (!gate.render) return;
			delta = gate.deltaTime;
		}
		lastRenderTime = now;
		if (enablePerformanceTracking) performance.mark("shader-cpu-start");
		const hasRealPointer = pointerX > -5;
		frameParams.deltaTime = delta;
		frameParams.pointer.x = hasRealPointer ? pointerX : .5;
		frameParams.pointer.y = hasRealPointer ? pointerY : .5;
		frameParams.pointer.seen = pointerSeen;
		frameParams.pointerActive = pointerActive;
		frameParams.dimensions.width = currentWidth;
		frameParams.dimensions.height = currentHeight;
		let built = null;
		let renderComp = null;
		try {
			runFrameSequence({
				updateDrivers: () => {
					updateMouseDrivers();
					updateAnimatedTime();
					updateChildBounds();
					runLayoutPassIfDirty();
				},
				ensureComposition: () => {
					if (structuralDirty || desiredHash === null) {
						desiredHash = computeStructuralHash();
						structuralDirty = false;
					}
					if (desiredHash === null) return false;
					built = pipelineCache.getOrBuild(desiredHash, () => buildComposition(desiredHash));
					bindComposition(built);
					return !!boundComposition;
				},
				beforeRender: () => runCallbacks(boundComposition.ir.onBeforeRender),
				flush: () => {
					writeSystemUniforms(boundComposition.store);
					boundComposition.store.flush();
				},
				render: () => {
					if (enablePerformanceTracking) performance.mark("shader-gpu-start");
					renderComp = pipelineCache.renderValue ?? built;
					if (renderComp?.broken) return false;
					const comp = renderComp;
					const scopeDevice = comp && comp.validationFrames > 0 && comp.ir.usesCustomWgsl ? root?.device : void 0;
					if (scopeDevice) scopeDevice.pushErrorScope("validation");
					try {
						comp?.passManager.render(context, frameParams, () => boundComposition?.store.flush());
					} finally {
						if (scopeDevice && comp) {
							comp.validationFrames--;
							scopeDevice.popErrorScope().then((error) => {
								if (error && !comp.broken) markBroken(comp, error.message);
							}).catch(() => {});
						}
					}
					if (enablePerformanceTracking) {
						performance.mark("shader-gpu-end");
						try {
							performance.measure("shader-gpu-time", "shader-gpu-start", "shader-gpu-end");
							const gpuMeasure = performance.getEntriesByName("shader-gpu-time")[0];
							if (gpuMeasure) performanceTracker.recordGpuTime(gpuMeasure.duration);
							performance.clearMeasures("shader-gpu-time");
							performance.clearMarks("shader-gpu-start");
							performance.clearMarks("shader-gpu-end");
						} catch {}
					}
				},
				markReady: () => {
					pipelineCache.markReady(desiredHash);
					if (firstRenderPending && !hasEmittedReady) {
						hasEmittedReady = true;
						firstRenderPending = false;
						if (onReadyCallback) queueMicrotask(onReadyCallback);
					}
				},
				afterRender: () => runCallbacks((renderComp ?? boundComposition).ir.onAfterRender)
			});
			consecutiveRenderErrors = 0;
		} catch (error) {
			noteRenderFailure(error);
			return;
		}
		if (enablePerformanceTracking) {
			performance.mark("shader-cpu-end");
			try {
				performance.measure("shader-cpu-time", "shader-cpu-start", "shader-cpu-end");
				const cpuMeasure = performance.getEntriesByName("shader-cpu-time")[0];
				if (cpuMeasure) performanceTracker.recordCpuTime(cpuMeasure.duration);
				performance.clearMarks("shader-cpu-start");
				performance.clearMarks("shader-cpu-end");
				performance.clearMeasures("shader-cpu-time");
			} catch {}
		}
		performanceTracker.recordFrame(performance.now() - now);
	};
	const renderFrame = () => renderFrameInternal(void 0);
	const renderAndWait = async (options) => {
		if (failureReason) return;
		if (!canRender()) throw new Error("Renderer is not ready");
		const wasAnimating = frameLoop.running;
		if (wasAnimating) stopAnimation();
		lastRenderTime = 0;
		renderFrameInternal(frameLocked ? 0 : void 0);
		if (root && options?.waitForGpu !== false) await awaitGpuIdle(root);
		if (wasAnimating) startAnimation();
	};
	const renderSyntheticFrame = async (deltaSeconds, options) => {
		if (!Number.isFinite(deltaSeconds)) throw new TypeError(`[gpu] renderSyntheticFrame: deltaSeconds must be a finite number, got ${deltaSeconds}`);
		renderFrameInternal(deltaSeconds);
		if (root && options?.waitForGpu !== false) await awaitGpuIdle(root);
	};
	const startAnimation = () => {
		if (failureReason || frameLocked || frameLoop.running || !shouldAnimate) return;
		performanceTracker.setRendering(true);
		frameLoop.start();
	};
	const stopAnimation = () => {
		if (!frameLoop.running) return;
		frameLoop.stop();
		performanceTracker.setRendering(false);
	};
	const updatePointerCoords = (event) => {
		if (!canvasElement) return;
		cachedCanvasRect = canvasElement.getBoundingClientRect();
		let clientX;
		let clientY;
		if ("touches" in event) {
			if (event.touches.length === 0) return;
			clientX = event.touches[0].clientX;
			clientY = event.touches[0].clientY;
		} else {
			clientX = event.clientX;
			clientY = event.clientY;
		}
		pointerX = (clientX - cachedCanvasRect.left) / cachedCanvasRect.width;
		pointerY = (clientY - cachedCanvasRect.top) / cachedCanvasRect.height;
		pointerSeen = true;
	};
	const onPointerMove = (event) => {
		if (!isInitialized || !canvasElement) return;
		updatePointerCoords(event);
		pointerActive = true;
		requestRender();
	};
	const onTouchMove = (event) => {
		if (!isInitialized || !canvasElement) return;
		updatePointerCoords(event);
		pointerActive = true;
		requestRender();
	};
	const onPointerUp = () => {
		if (!isInitialized) return;
		pointerActive = false;
		requestRender();
	};
	const onTouchEnd = () => {
		if (!isInitialized) return;
		pointerActive = false;
		requestRender();
	};
	const pressWithinCanvas = (clientX, clientY) => {
		if (!isInitialized || !canvasElement) return false;
		const rect = canvasElement.getBoundingClientRect();
		if (rect.width === 0 || rect.height === 0) return false;
		return clientX >= rect.left && clientX <= rect.right && clientY >= rect.top && clientY <= rect.bottom;
	};
	const onPointerDown = (event) => {
		if (event.button !== 0) return;
		if (!pressWithinCanvas(event.clientX, event.clientY)) return;
		pointerActive = true;
		updatePointerCoords(event);
		requestRender();
	};
	const onTouchStart = (event) => {
		if (event.touches.length === 0) return;
		if (!pressWithinCanvas(event.touches[0].clientX, event.touches[0].clientY)) return;
		pointerActive = true;
		updatePointerCoords(event);
		requestRender();
	};
	const updateMouseListenerRegistration = () => {
		let needsMouse = false;
		for (const node of nodes.values()) {
			if (node.usesPointer) {
				needsMouse = true;
				break;
			}
			if (shapeConfigUsesMouse(node.uniforms?.shape?.value) || listsUseMouse(node)) {
				needsMouse = true;
				break;
			}
			if (!node.metadata.maps) continue;
			for (const driver of Object.values(node.metadata.maps)) if (driver && typeof driver === "object" && "type" in driver) {
				const t = driver.type;
				if (t === "mouse" || t === "mouse-position") {
					needsMouse = true;
					break;
				}
			}
			if (needsMouse) break;
		}
		if (needsMouse && !mouseListenersNeeded) {
			mouseListenersNeeded = true;
			if (!globalEventUnregister && canvasElement) globalEventUnregister = globalEventManager.register({
				onMouseMove: onPointerMove,
				onTouchMove,
				onMouseDown: onPointerDown,
				onTouchStart,
				onMouseUp: onPointerUp,
				onTouchEnd
			});
		} else if (!needsMouse && mouseListenersNeeded) {
			mouseListenersNeeded = false;
			if (globalEventUnregister) {
				globalEventUnregister();
				globalEventUnregister = null;
			}
		}
	};
	const processQueuedRegistrations = () => {
		if (pendingRegistrationQueue.length === 0) return;
		const queue = [...pendingRegistrationQueue];
		pendingRegistrationQueue = [];
		for (const q of queue) registerNode(q.id, q.definition.fragment, q.parentId, q.metadata, q.uniforms, q.definition, q.domCanvas);
		const updates = pendingNodeUpdates;
		pendingNodeUpdates = [];
		for (const u of updates) if (u.kind === "uniform") updateUniformValue(u.id, u.name, u.value);
		else updateNodeMetadata(u.id, u.metadata);
	};
	const fail = (reason, error) => {
		if (failureReason) return;
		failureReason = reason;
		markGpuUnusable(reason);
		debugError(`[gpu] renderer stopped (${reason}):`, error);
		try {
			cleanup();
		} catch (e) {
			debugError("[gpu] cleanup during failure threw:", e);
		}
		if (!hasNotifiedUnavailable && onUnavailableCallback) {
			hasNotifiedUnavailable = true;
			const cb = onUnavailableCallback;
			queueMicrotask(() => {
				try {
					cb(reason, error);
				} catch (e) {
					debugError("[gpu] onUnavailable callback threw:", e);
				}
			});
		}
	};
	const noteRenderFailure = (error) => {
		if (isGpuUnavailableError(error)) {
			fail(error.reason, error);
			return;
		}
		consecutiveRenderErrors++;
		if (consecutiveRenderErrors === 1) authorError("[gpu] render error:", error);
		if (consecutiveRenderErrors >= MAX_CONSECUTIVE_RENDER_ERRORS) fail("render-failed", error);
	};
	const attachUncapturedErrorHandler = (device) => {
		if (!device || typeof device.addEventListener !== "function") return;
		detachUncapturedErrorHandler();
		uncapturedErrorDevice = device;
		uncapturedErrorHandler = (event) => {
			try {
				event.preventDefault();
			} catch {}
			const gpuError = event.error;
			authorError("[gpu] uncaptured device error (a custom WGSL body that fails to compile reports here — read the message for the line):", gpuError);
			if (typeof GPUOutOfMemoryError !== "undefined" && gpuError instanceof GPUOutOfMemoryError) {
				fail("out-of-memory", gpuError);
				return;
			}
			if (typeof GPUInternalError !== "undefined" && gpuError instanceof GPUInternalError) {
				fail("gpu-error", gpuError);
				return;
			}
			uncapturedValidationErrors++;
			if (uncapturedValidationErrors >= MAX_UNCAPTURED_VALIDATION_ERRORS) fail("gpu-error", gpuError);
		};
		device.addEventListener("uncapturederror", uncapturedErrorHandler);
	};
	const detachUncapturedErrorHandler = () => {
		if (uncapturedErrorDevice && uncapturedErrorHandler) try {
			uncapturedErrorDevice.removeEventListener("uncapturederror", uncapturedErrorHandler);
		} catch {}
		uncapturedErrorDevice = null;
		uncapturedErrorHandler = null;
	};
	const fireDeviceLost = (reason) => {
		if (deviceLostFired) return;
		deviceLostFired = true;
		try {
			stopAnimation();
		} catch {}
		if (onDeviceLostCallback) {
			const cb = onDeviceLostCallback;
			queueMicrotask(() => {
				try {
					cb(reason);
				} catch (e) {
					debugError("[gpu] onDeviceLost callback threw:", e);
				}
			});
			return;
		}
		fail("device-lost", reason);
	};
	const initialize = async (options) => {
		if (options.context !== void 0) throw new Error("[gpu] initialize({ context }) is not supported — this renderer is WebGPU-only. Remove the WebGL `context` option.");
		if (failureReason) return;
		const pageWide = getGpuUnusableReason();
		if (pageWide) {
			fail(pageWide);
			return;
		}
		if (isInitialized || isInitializing) return;
		const { canvas, resizeTarget, enablePerformanceTracking: enableTracking = false, colorSpace = "p3-linear", toneMapping = "linear", gpu, forceFullFrameRate: forceFullFps = false, observeElement: shouldObserveElement = true, timeOrigin: initialTimeOrigin } = options;
		if (initialTimeOrigin != null) sharedTimeOrigin = initialTimeOrigin;
		enablePerformanceTracking = enableTracking;
		forceFullFrameRate = forceFullFps;
		rendererColorSpace = colorSpace;
		rendererToneMapping = toneMapping;
		attachLayoutListeners();
		isInitializing = true;
		initializationAbortController = new AbortController();
		const abort = initializationAbortController;
		try {
			setColorSpaceModeFn = setColorSpaceModeGpu;
			setColorSpaceModeFn(colorSpace);
			if (abort.signal.aborted) return;
			canvasElement = canvas;
			rootCtx = await acquireRoot({
				device: gpu?.device,
				adapter: gpu?.adapter,
				onDeviceLost: (info) => fireDeviceLost(info.message || info.reason || "unknown")
			});
			if (abort.signal.aborted) return;
			root = rootCtx.root;
			attachUncapturedErrorHandler(root.device);
			pixelRatio = basePixelRatio();
			const lim = root.device.limits?.maxTextureDimension2D;
			if (typeof lim === "number" && lim > 0) cachedMaxTextureDim = lim;
			textureManager = createTextureManager(root);
			dispatcher = createComputeDispatcher(root);
			if (typeof window !== "undefined") {
				unloadHandler = () => cleanup();
				window.addEventListener("beforeunload", unloadHandler);
				windowResizeHandler = () => {
					if (lastRequestedWidth > 0 && lastRequestedHeight > 0) {
						const rawW = lastRequestedWidth;
						const rawH = lastRequestedHeight;
						lastRequestedWidth = 0;
						lastRequestedHeight = 0;
						updateRendererDimensions(rawW, rawH);
					}
				};
				window.addEventListener("resize", windowResizeHandler, { passive: true });
			}
			const rect = canvas.getBoundingClientRect();
			cachedCanvasRect = rect;
			isVisible = !shouldObserveElement || rect.width > 0 && rect.height > 0 && rect.top < window.innerHeight && rect.bottom > 0 && rect.left < window.innerWidth && rect.right > 0;
			shouldAnimate = true;
			const rawWidth = rect.width > 0 ? rect.width : canvas.width;
			const rawHeight = rect.height > 0 ? rect.height : canvas.height;
			const roundedRawWidth = Math.round(rawWidth);
			const roundedRawHeight = Math.round(rawHeight);
			if (roundedRawWidth > 0 && roundedRawHeight > 0) {
				lastRequestedWidth = roundedRawWidth;
				lastRequestedHeight = roundedRawHeight;
				const { width: cw, height: ch } = clampToTextureCap$1(roundedRawWidth, roundedRawHeight);
				currentWidth = cw;
				currentHeight = ch;
				logicalWidth = roundedRawWidth;
				logicalHeight = roundedRawHeight;
				hasInitialDimensions = true;
				setShaderCanvasDimensions(roundedRawWidth, roundedRawHeight);
				const { width: bufW, height: bufH } = bufferSize();
				canvas.width = bufW;
				canvas.height = bufH;
			} else hasInitialDimensions = false;
			context = configureCanvasContext(root, canvas);
			if (shouldObserveElement && typeof ResizeObserver !== "undefined") {
				const elementToObserve = resizeTarget ?? canvas.parentElement;
				if (elementToObserve) {
					resizeObserver = new ResizeObserver(handleResize);
					resizeObserver.observe(elementToObserve);
				} else console.warn("[gpu] no element available for resize observation — use resize() manually");
				if (typeof IntersectionObserver !== "undefined") {
					intersectionObserver = new IntersectionObserver(handleVisibilityChange, {
						threshold: 0,
						rootMargin: "128px"
					});
					intersectionObserver.observe(canvas);
				}
			}
			if (abort.signal.aborted) return;
			isInitialized = true;
			isRendererReady = true;
			processQueuedRegistrations();
			if (currentWidth > 0 && currentHeight > 0) renderFrame();
			if (shouldAnimate) startAnimation();
		} catch (error) {
			const reason = isGpuUnavailableError(error) ? error.reason : "init-failed";
			isInitializing = false;
			if (initializationAbortController === abort) initializationAbortController = null;
			fail(reason, error);
			return;
		} finally {
			isInitializing = false;
			if (initializationAbortController === abort) initializationAbortController = null;
		}
	};
	const cleanup = () => {
		if (initializationAbortController) initializationAbortController.abort();
		if (resizeObserver) {
			resizeObserver.disconnect();
			resizeObserver = null;
		}
		if (intersectionObserver) {
			intersectionObserver.disconnect();
			intersectionObserver = null;
		}
		if (globalEventUnregister) {
			globalEventUnregister();
			globalEventUnregister = null;
		}
		if (unloadHandler && typeof window !== "undefined") {
			window.removeEventListener("beforeunload", unloadHandler);
			unloadHandler = null;
		}
		if (windowResizeHandler && typeof window !== "undefined") {
			window.removeEventListener("resize", windowResizeHandler);
			windowResizeHandler = null;
		}
		if (pendingRenderRAF !== null && typeof cancelAnimationFrame === "function") {
			cancelAnimationFrame(pendingRenderRAF);
			pendingRenderRAF = null;
		}
		stopAnimation();
		detachUncapturedErrorHandler();
		unsubscribeNaturalSize?.();
		unsubscribeNaturalSize = null;
		if (typeof document !== "undefined" && document.fonts?.removeEventListener) document.fonts.removeEventListener("loadingdone", onFontsLoadedLayout);
		try {
			rootCtx?.release?.();
		} catch {}
		pipelineCache.clear();
		for (const c of [...liveCompositions]) disposeComposition(c);
		boundComposition = null;
		try {
			textureManager?.destroy();
		} catch {}
		try {
			if (rootCtx?.createdDevice) root?.destroy();
		} catch {}
		nodes.clear();
		rootId = null;
		parentToChildren.clear();
		pendingRegistrationQueue = [];
		pendingNodeUpdates = [];
		isRendererReady = false;
		pendingResize = null;
		isResizeScheduled = false;
		canvasElement = null;
		context = null;
		textureManager = null;
		dispatcher = null;
		root = null;
		rootCtx = null;
		isInitialized = false;
		isInitializing = false;
		hasInitialDimensions = false;
		initializationAbortController = null;
		currentWidth = 0;
		currentHeight = 0;
		logicalWidth = 0;
		logicalHeight = 0;
		desiredHash = null;
		structuralDirty = true;
		isVisible = false;
		shouldAnimate = true;
		frameLocked = false;
		pointerX = -10;
		pointerY = -10;
		pointerActive = false;
		pointerSeen = false;
		mouseListenersNeeded = false;
		onReadyCallback = null;
		hasEmittedReady = false;
		firstRenderPending = false;
		onDeviceLostCallback = null;
		deviceLostFired = false;
		uncapturedValidationErrors = 0;
		consecutiveRenderErrors = 0;
	};
	const getPerformanceStats = () => {
		const nodeCount = Math.max(0, nodes.size - 1);
		const rttNodeCount = boundComposition ? boundComposition.ir.rttPasses.length : 0;
		performanceTracker.updateNodeCounts(nodeCount, rttNodeCount);
		const rendererInfo = {
			render: { calls: boundComposition ? boundComposition.passManager.passCount : 0 },
			programs: { length: pipelineCache.size },
			memory: { textures: textureManager ? textureManager.textureCount : 0 }
		};
		return performanceTracker.getStats(rendererInfo);
	};
	return {
		initialize,
		cleanup,
		registerNode,
		removeNode,
		updateUniformValue,
		updateNodeMetadata,
		isInitialized: () => isInitialized,
		resize: (width, height) => {
			if (!root) return;
			updateRendererDimensions(width, height);
		},
		startAnimation,
		stopAnimation,
		renderAndWait,
		renderSyntheticFrame,
		getPerformanceStats,
		getLiveDriverValue: (nodeId, propName) => {
			const state = nodes.get(nodeId)?.mouseDriverState?.[propName];
			if (!state || state.liveX === void 0 || state.liveY === void 0) return null;
			return {
				x: state.liveX,
				y: state.liveY
			};
		},
		getLiveScalarValue: (nodeId, propName) => {
			const node = nodes.get(nodeId);
			if (!node) return null;
			const driver = node.metadata.maps?.[propName];
			if (driver?.type === "auto-animate") {
				const h = node._liveHandles?.[`_smoothed_${propName}`];
				const v = h instanceof FieldHandle ? h.value : void 0;
				return typeof v === "number" ? v : null;
			}
			if (driver?.type === "mouse") return node.mouseDriverState[propName]?.liveScalar ?? null;
			return null;
		},
		getNodeRegistry: () => ({ nodes: new Map(nodes) }),
		getRendererType: () => "webgpu",
		getInternalRenderer: () => root ? {
			device: root.device,
			adapter: rootCtx?.adapter,
			root
		} : null,
		setForceFullFrameRate: (enabled) => {
			const previous = forceFullFrameRate;
			forceFullFrameRate = enabled;
			return previous;
		},
		beginRecordingResolution: (targetWidth, targetHeight, ratio) => {
			if (!root || targetWidth <= 0 || targetHeight <= 0) return () => {};
			const prevRatio = pixelRatio;
			const prevWidth = currentWidth;
			const prevHeight = currentHeight;
			const prevLogicalWidth = logicalWidth;
			const prevLogicalHeight = logicalHeight;
			const prevLastRequestedWidth = lastRequestedWidth;
			const prevLastRequestedHeight = lastRequestedHeight;
			pixelRatio = ratio;
			applyRendererDimensions(targetWidth, targetHeight, targetWidth, targetHeight);
			return () => {
				if (!root) return;
				pixelRatio = prevRatio;
				applyRendererDimensions(prevWidth, prevHeight, prevLogicalWidth, prevLogicalHeight);
				lastRequestedWidth = prevLastRequestedWidth;
				lastRequestedHeight = prevLastRequestedHeight;
			};
		},
		setResolutionScale: (scale) => {
			if (!root) return;
			const clampedScale = Math.max(.05, Math.min(scale, 1));
			const targetRatio = Math.max(.01, basePixelRatio() * clampedScale);
			if (Math.abs(pixelRatio - targetRatio) < .001) return;
			pixelRatio = targetRatio;
			const logicalW = logicalWidth || currentWidth;
			const logicalH = logicalHeight || currentHeight;
			if (logicalW <= 0 || logicalH <= 0) return;
			const { width, height } = clampToTextureCap$1(logicalW, logicalH);
			applyRendererDimensions(width, height, logicalW, logicalH);
		},
		setFrameRateCap: (fps) => {
			frameIntervalCap = fps && fps > 0 ? 1e3 / Math.max(fps, 10) - 1 : 0;
		},
		setTimeOrigin: (origin) => {
			sharedTimeOrigin = origin;
		},
		setFrameLocked: (locked) => {
			const wasLocked = frameLocked;
			frameLocked = locked;
			if (!locked || wasLocked) return;
			stopAnimation();
			if (pendingRenderRAF !== null && typeof cancelAnimationFrame === "function") {
				cancelAnimationFrame(pendingRenderRAF);
				pendingRenderRAF = null;
			}
			globalElapsedTime = 0;
			for (const node of nodes.values()) {
				if (node.definition.animatedTime) {
					getAnimatedTimeState(animatedTimeKey(node)).value = 0;
					writeSyntheticScalar(node, "_animTime", 0);
				}
				const extras = node.definition.extraAnimatedTimes;
				if (extras) for (const [key, speedProp] of Object.entries(extras)) {
					getAnimatedTimeState(extraAnimatedTimeKey(node, speedProp)).value = 0;
					writeSyntheticScalar(node, `_animTime_${key}`, 0);
				}
			}
		},
		setOnReady: (callback) => {
			onReadyCallback = callback;
			if (callback && hasEmittedReady) queueMicrotask(callback);
		},
		setOnDeviceLost: (callback) => {
			onDeviceLostCallback = callback;
		},
		setOnUnavailable: (callback) => {
			onUnavailableCallback = callback;
			if (callback && failureReason && !hasNotifiedUnavailable) {
				hasNotifiedUnavailable = true;
				const reason = failureReason;
				queueMicrotask(() => {
					try {
						callback(reason);
					} catch (e) {
						debugError("[gpu] onUnavailable callback threw:", e);
					}
				});
			}
		},
		getFailureReason: () => failureReason,
		__testing: {
			needsTransformation,
			findChildNodes,
			buildFieldInits: (id) => {
				const node = nodes.get(id);
				return node ? buildFieldInits(node) : null;
			},
			ensureDriverState: (id) => {
				const node = nodes.get(id);
				if (node) ensureDriverState(node);
			},
			stepMouseDrivers: (deltaTime) => {
				frameParams.deltaTime = deltaTime;
				updateMouseDrivers();
			},
			stepAnimatedTime: (deltaTime) => {
				frameParams.deltaTime = deltaTime;
				updateAnimatedTime();
			},
			getAnimatedTimeValue: (id) => {
				const node = nodes.get(id);
				if (!node?.definition.animatedTime) return null;
				return getAnimatedTimeState(animatedTimeKey(node)).value;
			},
			writeExtraField: (id, name, value) => {
				const node = nodes.get(id);
				if (node) writeExtraFieldValue(node, name, value);
			},
			readExtraField: (id, name) => {
				const h = nodes.get(id)?._liveHandles?.[name];
				return h instanceof FieldHandle ? h.value : void 0;
			},
			getNodeRegistry: () => ({
				nodes,
				rootId,
				parentToChildren
			}),
			getParentToChildren: () => parentToChildren,
			getPendingRegistrations: () => [...pendingRegistrationQueue],
			getFrameDiagnostics: () => ({
				deltaTime: frameParams.deltaTime,
				pointerX,
				pointerY,
				pointerActive,
				globalElapsedTime,
				lastRenderTime,
				isVisible,
				forceFullFrameRate,
				sharedTimeOrigin,
				animating: frameLoop.running,
				mouseListenersNeeded
			}),
			computeStructuralHash,
			isStructuralDirty: () => structuralDirty,
			clearStructuralDirty: () => {
				structuralDirty = false;
			},
			setTestReady: (dims) => {
				isInitialized = true;
				isRendererReady = true;
				if (dims) {
					currentWidth = dims.width;
					currentHeight = dims.height;
					logicalWidth = dims.width;
					logicalHeight = dims.height;
					hasInitialDimensions = true;
				}
				processQueuedRegistrations();
			}
		}
	};
}
function createRendererFromJSON(preset, options) {
	const coreRenderer = shaderRendererGPU();
	const componentRegistry = /* @__PURE__ */ new Map();
	const nodeIdMap = /* @__PURE__ */ new Map();
	let presetCanvas = null;
	let frameLocked = false;
	getAllShaders().forEach((shader) => {
		componentRegistry.set(shader.definition.name, shader.definition);
	});
	for (const custom of options?.components ?? []) componentRegistry.set(custom.name, custom);
	async function initialize(canvas) {
		presetCanvas = canvas;
		await coreRenderer.initialize({
			canvas,
			enablePerformanceTracking: options?.enablePerformanceTracking || false,
			gpu: options?.gpu,
			forceFullFrameRate: options?.forceFullFrameRate || false
		});
		registerPresetNodes();
		coreRenderer.stopAnimation();
	}
	async function renderFrame(options$1) {
		const delta = options$1?.deltaSeconds;
		if (delta !== void 0 && !Number.isFinite(delta)) throw new TypeError(`[presetRenderer] renderFrame: deltaSeconds must be a finite number, got ${delta}`);
		if (!coreRenderer.isInitialized()) {
			debugWarn("[presetRenderer] Renderer not yet initialized, skipping frame");
			return;
		}
		if (delta === void 0) {
			if (frameLocked) throw new Error("[presetRenderer] renderFrame: this renderer is frame-locked (a previous call passed deltaSeconds); pass deltaSeconds on every call, or {deltaSeconds: 0} to repaint without advancing time");
			await coreRenderer.renderAndWait(options$1);
			return;
		}
		if (!frameLocked) {
			frameLocked = true;
			coreRenderer.setFrameLocked(true);
		}
		await coreRenderer.renderSyntheticFrame(delta, { waitForGpu: options$1?.waitForGpu });
	}
	function updatePreset(newPreset) {
		nodeIdMap.forEach((_, nodeId) => {
			coreRenderer.removeNode(nodeId);
		});
		nodeIdMap.clear();
		preset = newPreset;
		registerPresetNodes();
	}
	function getGPUContext() {
		const internal = coreRenderer.getInternalRenderer();
		return {
			type: "webgpu",
			device: internal?.device,
			adapter: internal?.adapter,
			canvas: presetCanvas
		};
	}
	function dispose() {
		coreRenderer.cleanup();
		frameLocked = false;
	}
	function registerPresetNodes() {
		const rootId = "preset-root";
		coreRenderer.registerNode(rootId, rootPassthrough.fragment, null, null, {}, rootPassthrough);
		preset.components.forEach((component, index) => {
			registerComponent(component, rootId, index);
		});
	}
	function registerComponent(component, parentId, renderOrder) {
		const componentDef = componentRegistry.get(component.type) ?? getRegisteredShader(component.type);
		if (!componentDef) {
			console.warn(`[createRendererFromJSON] Unknown component type: ${component.type}`);
			return;
		}
		const nodeId = component.id || `${component.type}_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;
		nodeIdMap.set(nodeId, component.type);
		const uniforms = createGpuUniformsMap(componentDef, Object.fromEntries(Object.entries(componentDef.props).map(([key, propConfig]) => [key, component.props?.[key] !== void 0 ? component.props[key] : propConfig.default])), nodeId);
		const metadata = {
			blendMode: component.props?.blendMode || "normal",
			opacity: component.props?.opacity,
			visible: component.props?.visible,
			renderOrder,
			id: component.id,
			mask: component.props?.maskSource ? {
				source: component.props.maskSource,
				type: component.props.maskType || "alpha"
			} : void 0,
			transform: component.props?.transform ? {
				offsetX: 0,
				offsetY: 0,
				rotation: 0,
				scale: 1,
				anchorX: .5,
				anchorY: .5,
				edges: "transparent",
				...component.props.transform
			} : void 0
		};
		coreRenderer.registerNode(nodeId, componentDef.fragment, parentId, metadata, uniforms, componentDef);
		component.children?.forEach((child, index) => {
			registerComponent(child, nodeId, index);
		});
	}
	return {
		initialize,
		renderFrame,
		updatePreset,
		getGPUContext,
		dispose,
		setOnUnavailable: coreRenderer.setOnUnavailable,
		getFailureReason: coreRenderer.getFailureReason
	};
}
function resolveMaskDependencies(nodes) {
	const elementIdToNodeId = /* @__PURE__ */ new Map();
	const maskDependencies = /* @__PURE__ */ new Map();
	const result = {
		processingOrder: [],
		maskSources: /* @__PURE__ */ new Map(),
		circularDependencies: []
	};
	nodes.forEach((node) => {
		if (node.elementId) elementIdToNodeId.set(node.elementId, node.id);
	});
	nodes.forEach((node) => {
		if (node.mask?.source) {
			const sourceElementId = node.mask.source.startsWith("#") ? node.mask.source.substring(1) : node.mask.source;
			const sourceNodeId = elementIdToNodeId.get(sourceElementId);
			if (sourceNodeId) {
				if (!maskDependencies.has(node.id)) maskDependencies.set(node.id, []);
				maskDependencies.get(node.id)?.push(sourceNodeId);
				result.maskSources.set(node.id, sourceNodeId);
			} else console.warn(`Mask source "${sourceElementId}" not found for node "${node.id}"`);
		}
	});
	result.processingOrder = topologicalSort(maskDependencies, result.circularDependencies);
	return result;
}
function topologicalSort(dependencies, circularDependencies) {
	const result = [];
	const visited = /* @__PURE__ */ new Set();
	const tempVisited = /* @__PURE__ */ new Set();
	function visit(nodeId, path = []) {
		if (visited.has(nodeId)) return;
		if (tempVisited.has(nodeId)) {
			const cycle = [...path.slice(path.indexOf(nodeId)), nodeId];
			circularDependencies.push(cycle);
			return;
		}
		tempVisited.add(nodeId);
		path.push(nodeId);
		const deps = dependencies.get(nodeId) || [];
		for (const depId of deps) visit(depId, [...path]);
		tempVisited.delete(nodeId);
		visited.add(nodeId);
		result.push(nodeId);
	}
	for (const nodeId of dependencies.keys()) if (!visited.has(nodeId)) visit(nodeId);
	const allSourceNodeIds = new Set([...dependencies.values()].flat());
	for (const sourceId of allSourceNodeIds) if (!visited.has(sourceId) && !dependencies.has(sourceId)) result.push(sourceId);
	return result.reverse();
}
const screenUVToBoxLocal = (sx, sy, g) => {
	const th = g.rotationDeg * Math.PI / 180;
	const c = Math.cos(th);
	const s = Math.sin(th);
	const sqx = (sx - g.centerX) * g.aspectRatio;
	const sqy = sy - g.centerY;
	const rx = sqx * c + sqy * s;
	const ry = sqx * -s + sqy * c;
	const hwSq = Math.max(g.halfWidthUV * g.aspectRatio, 1e-6);
	const hhSq = Math.max(g.halfHeightUV, 1e-6);
	return {
		x: rx / (2 * hwSq) + .5,
		y: ry / (2 * hhSq) + .5
	};
};
const boxLocalToScreenUV = (bx, by, g) => {
	const th = g.rotationDeg * Math.PI / 180;
	const c = Math.cos(th);
	const s = Math.sin(th);
	const lx = (bx - .5) * 2 * (g.halfWidthUV * g.aspectRatio);
	const ly = (by - .5) * 2 * g.halfHeightUV;
	const sqx = lx * c - ly * s;
	const sqy = lx * s + ly * c;
	return {
		x: sqx / g.aspectRatio + g.centerX,
		y: sqy + g.centerY
	};
};
export { DEFAULT_BOUNDING_BOX, GpuUnavailableError, MAX_COLOR_STOPS, SHAPE3D_DEFAULTS, SHAPE3D_TYPES, SVG3D_DEFAULTS, SVG3D_TYPES, anchorOffsetForBlockTopLeft, authorError, boxLocalToScreenUV, computeBoundsShift, computeGroupBlockRects, computeLayout, computeTreeLayout, createGpuUniformsMap, createRendererFromJSON, debugError, debugWarn, destroyDefaultGpuDevice, getNaturalSize, getRegisteredShader, getRegisteredShaders, getWebGPUSupport, hasCustomWgsl, is3dShapeType, isDimensionalValue, isGpuUnavailableError, isShadersDebug, isSvg3dShapeType, isVolumetricShapeType, isWebGPUSupported, layoutBlockSize, measureChild, measureNode, measureText, measureTextBlock, onNaturalSizeChange, onShaderRegistered, registerNaturalSize, registerShader, resolveBoundingBox, resolveLayoutConfig, resolveMaskDependencies, resolveStops, rootPassthrough, screenUVToBoxLocal, setShadersDebug, shaderRendererGPU, shape3dBoundingRadius, sortStops, unregisterShader, wrapLines };
