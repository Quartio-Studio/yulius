import { $f as f32, Ac as defineStd, C as fitIsoGrid, Jf as textureStorage2d, Kf as atomic, M as force, N as integrator, S as expDecay, Sl as resolveRenderRes, Wf as vec4f, Xf as struct, Yf as arrayOf, _ as agentSim, bl as makeCpuValueGetter, ep as u32, gu as pointShapeOptions, k as channel, mf as DEG_TO_RAD, qf as texture2d, v as renderAgents, xf as typegpu_default, xl as readAgentFrame, y as cameraRowsYDown } from "./std-ClftfWpF.js";
var GRID_DIM_MAX = 256;
var MAX_PARTICLES = GRID_DIM_MAX * GRID_DIM_MAX;
var COUNT_MAX = 4e4;
var COUNT_MAX_MOBILE = 12e3;
var OUT_MAX = resolveRenderRes({
	desktop: 1024,
	mobile: 768
});
var OUT_CELLS = OUT_MAX * OUT_MAX;
var SPLAT_R = 8;
var AA_W = .75;
var MIN_ALPHA = .002;
var STATE_FORMAT = "rgba16float";
var FP = 2048;
var WOBBLE_FREQ = 1.15;
var ALPHA_K = 2.6;
var PERSP_K = 1.1;
var STIFF = 26;
var DRAG = 4.2;
var MAX_SPEED = 4;
var CURSOR_FORCE_K = 10;
var CURSOR_ZFORCE_K = 4;
var SimParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	rowX: vec4f,
	rowY: vec4f,
	rowZ: vec4f,
	dt: f32,
	time: f32,
	snap: f32,
	gridW: f32,
	gridH: f32,
	outW: f32,
	outH: f32,
	depth: f32,
	dragMul: f32,
	wobbleAmp: f32,
	depthShading: f32,
	spacing: f32,
	particleSize: f32,
	aspect: f32,
	zoom: f32,
	transX: f32,
	transY: f32,
	pointerX: f32,
	pointerY: f32,
	cursorForce: f32,
	cursorZForce: f32,
	cursorRadSq: f32
}), "SimParams");
var fieldLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	pos: {
		storage: arrayOf(vec4f, MAX_PARTICLES),
		access: "mutable"
	},
	vel: {
		storage: arrayOf(vec4f, MAX_PARTICLES),
		access: "mutable"
	},
	col: {
		storage: arrayOf(vec4f, MAX_PARTICLES),
		access: "mutable"
	},
	accumR: {
		storage: arrayOf(atomic(u32), OUT_CELLS),
		access: "mutable"
	},
	accumG: {
		storage: arrayOf(atomic(u32), OUT_CELLS),
		access: "mutable"
	},
	accumB: {
		storage: arrayOf(atomic(u32), OUT_CELLS),
		access: "mutable"
	},
	accumW: {
		storage: arrayOf(atomic(u32), OUT_CELLS),
		access: "mutable"
	},
	params: { uniform: SimParams },
	src: { texture: texture2d(f32) },
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "fieldLayout");
var DEPTH_SOURCE_MODES = {
	luminance: 0,
	luminanceInverted: 1,
	red: 2,
	green: 3,
	blue: 4,
	saturation: 5,
	alpha: 6
};
function makeParticleFieldSimKernel(channelMode) {
	return integrator.relief(fieldLayout, {
		channel: channel.byMode[channelMode] ?? channel.luminance,
		cursor: force.cursorInView(fieldLayout, { perspK: PERSP_K }),
		stiffness: STIFF,
		maxSpeed: MAX_SPEED,
		minAlpha: MIN_ALPHA,
		name: "particleFieldSim"
	});
}
function particleFieldRender(shape, out) {
	return renderAgents.relief(fieldLayout, {
		shape,
		splatRCap: SPLAT_R,
		aaW: AA_W,
		minAlpha: MIN_ALPHA,
		fp: FP,
		wobbleFreq: WOBBLE_FREQ,
		perspK: PERSP_K,
		out,
		weightedColor: { alphaK: ALPHA_K },
		names: {
			splat: "particleFieldSplat",
			resolve: "particleFieldResolve"
		}
	});
}
const makeParticleFieldSplatKernel = (shape) => particleFieldRender(shape, [OUT_MAX, OUT_MAX]).splat.kernel;
const particleFieldSplatKernel = makeParticleFieldSplatKernel("dot");
const particleFieldResolveKernel = particleFieldRender("dot", [OUT_MAX, OUT_MAX]).resolve.kernel;
const componentDefinition = defineStd({
	name: "ParticleField",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Explodes the child content into a breathing 3D field of particles — bright areas float toward you and dark areas recede, the whole relief can be orbited with the camera rotations, and the cursor physically throws particles that spring back home",
	requiresRTT: true,
	requiresChild: true,
	usesPointer: true,
	props: {
		count: {
			default: 15e3,
			description: "Number of particles — how finely the child is sampled into the field",
			ui: {
				type: "range",
				min: 1e3,
				max: COUNT_MAX,
				step: 500,
				label: "Count",
				group: "Field"
			}
		},
		depthSource: {
			default: "luminance",
			compileTime: true,
			description: "Which channel of the child drives each particle's depth toward or away from the camera",
			ui: {
				type: "select",
				options: [
					{
						value: "luminance",
						label: "Luminance"
					},
					{
						value: "luminanceInverted",
						label: "Luminance (Inverted)"
					},
					{
						value: "red",
						label: "Red"
					},
					{
						value: "green",
						label: "Green"
					},
					{
						value: "blue",
						label: "Blue"
					},
					{
						value: "saturation",
						label: "Saturation"
					},
					{
						value: "alpha",
						label: "Alpha"
					}
				],
				label: "Depth Source",
				group: "Field"
			}
		},
		depth: {
			default: .7,
			description: "Depth of the relief — how far bright and dark areas rise toward and sink away from the camera",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Depth",
				group: "Field"
			}
		},
		particleShape: {
			default: "dot",
			compileTime: true,
			description: "What each particle is drawn as — a crisp dot or square, or a soft glow puff",
			ui: {
				type: "select",
				options: pointShapeOptions,
				label: "Shape",
				group: "Look"
			}
		},
		particleSize: {
			default: .75,
			description: "Size of each particle relative to the grid spacing",
			ui: {
				type: ["range", "map"],
				min: .3,
				max: 3,
				step: .01,
				label: "Particle Size",
				group: "Look"
			}
		},
		depthShading: {
			default: .6,
			description: "How much far particles dim into the distance — the main cue that sells the 3D relief",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Depth Shading",
				group: "Look"
			}
		},
		wobble: {
			default: .35,
			description: "Gentle idle drift that keeps the field breathing even on a still image",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Wobble",
				group: "Motion"
			}
		},
		zoom: {
			default: 1,
			description: "Camera distance — dolly the whole field closer or further away",
			ui: {
				type: ["range", "map"],
				min: .3,
				max: 2.5,
				step: .01,
				label: "Zoom",
				group: "Camera"
			}
		},
		rotationX: {
			default: 0,
			description: "Camera orbit around the horizontal axis — tilt the relief toward or away from you",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Rotate X",
				group: "Camera"
			}
		},
		rotationY: {
			default: 0,
			description: "Camera orbit around the vertical axis — view the particle extrusion from the side",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Rotate Y",
				group: "Camera"
			}
		},
		rotationZ: {
			default: 0,
			description: "Camera roll around the view axis",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Rotate Z",
				group: "Camera"
			}
		},
		offsetX: {
			default: 0,
			description: "Shift the whole field horizontally across the frame (in screen widths)",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Offset X",
				group: "Camera"
			}
		},
		offsetY: {
			default: 0,
			description: "Shift the whole field vertically across the frame (in screen heights) — tilt it flat, then sink it toward the bottom like a ground plane",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Offset Y",
				group: "Camera"
			}
		},
		cursorMode: {
			default: "push",
			description: "How the cursor disturbs the field — push throws particles away and toward you, pull gathers them in",
			ui: {
				type: "select",
				options: [
					{
						value: "none",
						label: "None"
					},
					{
						value: "push",
						label: "Push"
					},
					{
						value: "pull",
						label: "Pull"
					}
				],
				label: "Cursor Mode",
				group: "Interaction"
			}
		},
		cursorStrength: {
			default: .8,
			description: "Strength of the cursor force — how hard particles get thrown before springing back",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Cursor Strength",
				group: "Interaction"
			}
		},
		cursorRadius: {
			default: .2,
			description: "Reach of the cursor field, in screen units",
			ui: {
				type: "range",
				min: .05,
				max: 1,
				step: .01,
				label: "Cursor Radius",
				group: "Interaction"
			}
		}
	},
	...agentSim({
		layout: fieldLayout,
		params: SimParams,
		maxAgents: MAX_PARTICLES,
		countCap: {
			desktop: COUNT_MAX,
			mobile: COUNT_MAX_MOBILE
		},
		externalKeys: ["src"],
		childTexture: { externalKey: "src" },
		output: {
			key: "outTex",
			name: "field",
			size: [OUT_MAX, OUT_MAX],
			format: STATE_FORMAT
		},
		bake: (params) => {
			const { getCpuValue, dimensions } = params;
			const aspect0 = dimensions.height > 0 ? dimensions.width / dimensions.height : 1;
			let outW = OUT_MAX;
			let outH = OUT_MAX;
			if (aspect0 >= 1) outH = Math.max(Math.round(OUT_MAX / aspect0), 1);
			else outW = Math.max(Math.round(OUT_MAX * aspect0), 1);
			const channelMode = DEPTH_SOURCE_MODES[getCpuValue("depthSource") || "luminance"] ?? 0;
			const render = particleFieldRender(getCpuValue("particleShape") || "dot", [outW, outH]);
			return {
				pipelines: {
					sim: {
						kernel: makeParticleFieldSimKernel(channelMode),
						threads: "grid"
					},
					splat: render.splat,
					resolve: render.resolve
				},
				program: [
					"sim",
					"splat",
					"resolve"
				],
				outputSize: [outW, outH],
				setup: {
					outW,
					outH,
					aspect0
				}
			};
		},
		frame: (sys, { getCpuValue }, setupRaw) => {
			const { outW, outH, aspect0 } = setupRaw;
			const g = makeCpuValueGetter(getCpuValue);
			let localTime = 0;
			let frameIdx = 0;
			const DEG_TO_RAD$1 = DEG_TO_RAD;
			return (frameParams) => {
				const frame = readAgentFrame(frameParams, aspect0);
				const dt = frame.dt;
				localTime += dt;
				const aspect = frame.aspect;
				const count = sys.resolveCount(g("count", 15e3), 500);
				const { w: GW, h: GH } = fitIsoGrid(count, aspect, 8, GRID_DIM_MAX);
				const cursorMode = getCpuValue("cursorMode") || "push";
				const cursorSign = cursorMode === "pull" ? -1 : 1;
				const cursorStrength = cursorMode === "none" ? 0 : g("cursorStrength", .5);
				const cursorRadius = g("cursorRadius", .22);
				const rows = cameraRowsYDown(g("rotationX", 0) * DEG_TO_RAD$1, g("rotationY", 0) * DEG_TO_RAD$1, g("rotationZ", 0) * DEG_TO_RAD$1);
				sys.writeParams({
					rowX: vec4f(rows.rowX[0], rows.rowX[1], rows.rowX[2], 0),
					rowY: vec4f(rows.rowY[0], rows.rowY[1], rows.rowY[2], 0),
					rowZ: vec4f(rows.rowZ[0], rows.rowZ[1], rows.rowZ[2], 0),
					dt,
					time: localTime,
					snap: frameIdx === 0 ? 1 : 0,
					gridW: GW,
					gridH: GH,
					outW,
					outH,
					depth: g("depth", .7),
					dragMul: expDecay(DRAG, dt),
					wobbleAmp: g("wobble", .35),
					depthShading: Math.min(Math.max(g("depthShading", .6), 0), 1),
					spacing: Math.min(outW / GW, outH / GH),
					particleSize: g("particleSize", 1.3),
					aspect,
					zoom: Math.min(Math.max(g("zoom", 1), .05), 10),
					transX: g("offsetX", 0),
					transY: g("offsetY", 0),
					pointerX: frame.pointerX,
					pointerY: frame.pointerY,
					cursorForce: cursorSign * cursorStrength * CURSOR_FORCE_K,
					cursorZForce: cursorSign * cursorStrength * CURSOR_ZFORCE_K,
					cursorRadSq: cursorRadius * cursorRadius
				});
				frameIdx++;
				return sys.frame({
					count,
					grid: [GW, GH]
				});
			};
		},
		fragment: {
			output: "field",
			fallback: "child"
		}
	})
});
var ParticleField_default = componentDefinition;
export { particleFieldResolveKernel as a, makeParticleFieldSplatKernel as i, componentDefinition as n, particleFieldSplatKernel as o, makeParticleFieldSimKernel as r, ParticleField_default as t };
