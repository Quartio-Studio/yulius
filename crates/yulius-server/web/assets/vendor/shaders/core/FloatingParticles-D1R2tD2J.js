import { $f as f32, A as drift, Ac as defineStd, F as pointSlot, Hd as call, Jf as textureStorage2d, Kf as atomic, M as force, N as integrator, S as expDecay, Sl as resolveRenderRes, Wf as vec4f, Xf as struct, Yf as arrayOf, _ as agentSim, _u as straightAlphaOver, bl as makeCpuValueGetter, ep as u32, gu as pointShapeOptions, hf as PI, mf as DEG_TO_RAD$1, mu as makeUniformScatterInit, v as renderAgents, xf as typegpu_default, xl as readAgentFrame, yl as SIZE_REF_RES } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
var MAX_MOTES = 8192;
var COUNT_MAX_MOBILE = 3e3;
var RES = resolveRenderRes({
	desktop: 1024,
	mobile: 512
});
var STATE_FORMAT = "rgba16float";
var DEG_TO_RAD = DEG_TO_RAD$1;
var BASE_DRIFT = .12;
var ORBIT_RAD = .035;
var ORBIT_RATE = 1.4;
var TWINKLE_FREQ = 2;
var CURSOR_FORCE_K = 3.5;
var CURSOR_DRAG = 1.1;
var ANGLE_FLIP = PI;
var SimParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	color: vec4f,
	count: f32,
	dt: f32,
	time: f32,
	aspect: f32,
	domainX: f32,
	driftBase: f32,
	angleRad: f32,
	speedVar: f32,
	angleVarRad: f32,
	randomness: f32,
	twinkle: f32,
	softness: f32,
	bodyR: f32,
	cursorX: f32,
	cursorY: f32,
	cursorRadSq: f32,
	cursorForce: f32,
	dragMul: f32
}), "SimParams");
var simLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	agents: {
		storage: arrayOf(vec4f, MAX_MOTES),
		access: "mutable"
	},
	accumE: {
		storage: arrayOf(atomic(u32), RES * RES),
		access: "mutable"
	},
	params: { uniform: SimParams },
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "simLayout");
const floatingParticlesInitKernel = makeUniformScatterInit(simLayout, "floatingParticlesInit");
const floatingParticlesUpdateKernel = integrator.drift2d(simLayout, {
	drift: drift.variedHeading(simLayout),
	gust: force.cursorGust(simLayout),
	name: "floatingParticlesUpdate"
});
function floatingParticlesRender(shape) {
	return renderAgents.pointWorld(simLayout, {
		shape,
		res: RES,
		place: pointSlot.orbitalPlace(simLayout, {
			radius: ORBIT_RAD,
			rate: ORBIT_RATE
		}),
		brightness: pointSlot.twinkleBrightness(simLayout, { freq: TWINKLE_FREQ }),
		bodyRadius: pointSlot.presenceRadius(simLayout),
		names: {
			splat: "floatingParticlesSplat",
			resolve: "floatingParticlesResolve"
		}
	});
}
const makeFloatingParticlesSplatKernel = (shape) => floatingParticlesRender(shape).splat.kernel;
const floatingParticlesSplatKernel = makeFloatingParticlesSplatKernel("dot");
const floatingParticlesResolveKernel = floatingParticlesRender("dot").resolve.kernel;
const particleOverChild = straightAlphaOver;
const componentDefinition = defineStd({
	name: "FloatingParticles",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Textures",
	description: "Drifting, twinkling motes — thousands of real simulated particles floating in a shared heading with per-particle wander and variance, and a cursor that stirs the field with a gust that settles back into the drift",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	props: {
		particleColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "Color of the particles",
			ui: {
				type: "color",
				label: "Particle Color",
				group: "Colors"
			}
		},
		shape: {
			default: "dot",
			compileTime: true,
			description: "What each particle is drawn as — a crisp dot or square (softness still feathers them), or a soft glow puff",
			ui: {
				type: "select",
				options: pointShapeOptions,
				label: "Shape",
				group: "Effect"
			}
		},
		count: {
			default: 1200,
			description: "Number of particles",
			ui: {
				type: "range",
				min: 100,
				max: MAX_MOTES,
				step: 100,
				label: "Count",
				group: "Effect"
			}
		},
		particleSize: {
			default: 1.2,
			description: "Size of the particles (each also varies slightly for depth)",
			ui: {
				type: ["range", "map"],
				min: .3,
				max: 4,
				step: .05,
				label: "Particle Size",
				group: "Effect"
			}
		},
		softness: {
			default: .1,
			description: "Edge softness of each particle — 0 is a crisp shape, 1 a soft glow puff",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		speed: {
			default: .25,
			description: "Speed of the shared drift",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		angle: {
			default: 90,
			description: "Drift heading in degrees (0 = left, 90 = up, 180 = right, 270 = down — the classic default floats upward)",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Animation"
			}
		},
		speedVariance: {
			default: .3,
			description: "Per-particle speed variance around the shared drift (0 = everything moves in lockstep)",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .05,
				label: "Speed Variance",
				group: "Animation"
			}
		},
		angleVariance: {
			default: 30,
			description: "Per-particle heading variance in degrees (0 = one shared direction, 180 = every which way)",
			ui: {
				type: "range",
				min: 0,
				max: 180,
				step: 1,
				label: "Angle Variance",
				group: "Animation"
			}
		},
		randomness: {
			default: .25,
			description: "Orbital wander — each particle circles lazily around its drift path instead of gliding straight",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .05,
				label: "Randomness",
				group: "Animation"
			}
		},
		twinkle: {
			default: .5,
			description: "Intensity of the twinkle (0 = steady, 1 = full shimmer)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .05,
				label: "Twinkle",
				group: "Animation"
			}
		},
		cursorStrength: {
			default: 0,
			description: "How strongly the cursor stirs the field — 0 (default) leaves the drift undisturbed",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Cursor Gust",
				group: "Interaction"
			}
		}
	},
	...agentSim({
		layout: simLayout,
		params: SimParams,
		maxAgents: MAX_MOTES,
		countCap: {
			desktop: MAX_MOTES,
			mobile: COUNT_MAX_MOBILE
		},
		output: {
			key: "outTex",
			name: "particleTexture",
			size: [RES, RES],
			format: STATE_FORMAT
		},
		bake: ({ getCpuValue }) => {
			const render = floatingParticlesRender(getCpuValue("shape") || "dot");
			return {
				pipelines: {
					init: {
						kernel: floatingParticlesInitKernel,
						threads: "max"
					},
					update: {
						kernel: floatingParticlesUpdateKernel,
						threads: "agents"
					},
					splat: render.splat,
					resolve: render.resolve
				},
				initStep: "init",
				program: [
					"update",
					"splat",
					"resolve"
				]
			};
		},
		frame: (sys, { getCpuValue }) => {
			const g = makeCpuValueGetter(getCpuValue);
			let localTime = 0;
			return (frameParams) => {
				const { dt, aspect, pointerX, pointerY } = readAgentFrame(frameParams);
				localTime += dt;
				const domainX = Math.max(aspect, .01);
				const count = sys.resolveCount(g("count", 1200));
				const cursorX = pointerX * aspect;
				const cursorY = pointerY;
				const cursorRadius = .22;
				const col = getCpuValue("particleColor");
				const bodyR = Math.min(Math.max(g("particleSize", 1.2), .3), 4) * 2 / SIZE_REF_RES;
				sys.writeParams({
					color: vec4f(col?.x ?? 1, col?.y ?? 1, col?.z ?? 1, col?.w ?? 1),
					count,
					dt,
					time: localTime,
					aspect,
					domainX,
					driftBase: Math.min(Math.max(g("speed", .25), 0), 1.5) * BASE_DRIFT / .25,
					angleRad: g("angle", 90) * DEG_TO_RAD + ANGLE_FLIP,
					speedVar: Math.min(Math.max(g("speedVariance", .3), 0), 1),
					angleVarRad: Math.min(Math.max(g("angleVariance", 30), 0), 180) * DEG_TO_RAD,
					randomness: Math.min(Math.max(g("randomness", .25), 0), 1),
					twinkle: Math.min(Math.max(g("twinkle", .5), 0), 1),
					softness: Math.min(Math.max(g("softness", .1), 0), 1),
					bodyR,
					cursorX,
					cursorY,
					cursorRadSq: cursorRadius * cursorRadius,
					cursorForce: Math.min(Math.max(g("cursorStrength", 0), 0), 1) * CURSOR_FORCE_K,
					dragMul: expDecay(CURSOR_DRAG, dt)
				});
				return sys.frame({ count });
			};
		},
		fragment: {
			output: "particleTexture",
			fallback: "child",
			compose: (field, { childNode }) => childNode ? call(particleOverChild, "particleOverChild", [field, childNode]) : field
		}
	})
});
var FloatingParticles_default = componentDefinition;
export { floatingParticlesSplatKernel as a, particleOverChild as c, floatingParticlesResolveKernel as i, componentDefinition as n, floatingParticlesUpdateKernel as o, floatingParticlesInitKernel as r, makeFloatingParticlesSplatKernel as s, FloatingParticles_default as t };
