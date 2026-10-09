import { $f as f32, Ac as defineStd, Jf as textureStorage2d, Kf as atomic, M as force, N as integrator, Sl as resolveRenderRes, Wf as vec4f, Xf as struct, Yf as arrayOf, _ as agentSim, bl as makeCpuValueGetter, du as makeClusterFormationInit, ep as u32, hu as orientedShapeOptions, v as renderAgents, xf as typegpu_default, xl as readAgentFrame, yl as SIZE_REF_RES } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var MAX_AGENTS = 4096;
var COUNT_MAX_MOBILE = 1200;
var RES = resolveRenderRes({
	desktop: 1024,
	mobile: 640
});
var STATE_FORMAT = "rgba16float";
var SPLAT_R = 16;
var BASE_SPEED = .13;
var FORCE_RATIO = 3.5;
var AGIT_REST = 1.5;
var AGIT_GAIN = .4545;
var AGIT_COOL = 1.3;
var SimParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	colA: vec4f,
	colB: vec4f,
	count: f32,
	trails: f32,
	dt: f32,
	aspect: f32,
	domainX: f32,
	maxSpeed: f32,
	maxForce: f32,
	perceptionSq: f32,
	sepRadiusSq: f32,
	sepW: f32,
	aliW: f32,
	cohW: f32,
	cursorX: f32,
	cursorY: f32,
	cursorMode: f32,
	cursorRadius: f32,
	cursorRadiusSq: f32,
	cursorForce: f32,
	bodyR: f32,
	margin: f32,
	turnForce: f32,
	seed: f32
}), "SimParams");
var simLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	agents: {
		storage: arrayOf(vec4f, MAX_AGENTS),
		access: "mutable"
	},
	agit: {
		storage: arrayOf(f32, MAX_AGENTS),
		access: "mutable"
	},
	accumE: {
		storage: arrayOf(atomic(u32), RES * RES),
		access: "mutable"
	},
	accumS: {
		storage: arrayOf(atomic(u32), RES * RES),
		access: "mutable"
	},
	trailBuf: {
		storage: arrayOf(vec4f, RES * RES),
		access: "mutable"
	},
	params: { uniform: SimParams },
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "simLayout");
const boidsInitKernel = makeClusterFormationInit(simLayout, "boidsInit");
const boidsUpdateKernel = integrator.steering2d(simLayout, {
	forces: [
		force.flocking(simLayout, { maxAgents: MAX_AGENTS }),
		force.cursorSteer(simLayout),
		force.wallTurn(simLayout)
	],
	agitation: {
		rest: AGIT_REST,
		gain: AGIT_GAIN,
		cool: AGIT_COOL
	},
	cruiseFloor: .35,
	wallRestitution: .6,
	name: "boidsUpdate"
});
function boidsRender(shape, colorSpaceMode) {
	return renderAgents.orientedWorld(simLayout, {
		shape,
		res: RES,
		splatRCap: SPLAT_R,
		heading: "velocity",
		agitation: "buffer",
		comet: true,
		ramp: {
			colorSpace: colorSpaceMode,
			trails: "on"
		},
		names: {
			splat: "boidsSplat",
			resolve: "boidsResolve"
		}
	});
}
const makeBoidsSplatKernel = (shape) => boidsRender(shape, 2).splat.kernel;
const makeBoidsResolveKernel = (mode) => boidsRender("arrow", mode).resolve.kernel;
const boidsSplatKernel = makeBoidsSplatKernel("arrow");
const boidsResolveKernel = makeBoidsResolveKernel(2);
const componentDefinition = defineStd({
	name: "Boids",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "A living murmuration of hundreds of flocking agents drawn as crisp arrows, streaks, dots or glowing comets — separation, alignment and cohesion drive fluid, splitting-and-merging ribbons, agents flash toward an excited color when scattered, and the cursor acts as an attractor or a predator",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	props: {
		colorA: {
			default: "#8ec5ff",
			transform: transformColor,
			description: "Color of agents cruising calmly with the flock",
			ui: {
				type: "color",
				label: "Rest Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ff7ad9",
			transform: transformColor,
			description: "Color agents flash toward when agitated — scattering from the predator cursor or banking through a hard turn",
			ui: {
				type: "color",
				label: "Excited Color",
				group: "Colors"
			}
		},
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for the rest→excited color ramp",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		agentShape: {
			default: "arrow",
			compileTime: true,
			description: "What each agent is drawn as — a hard-edged arrow, streak or square pointing along its heading, a plain dot, or a soft glowing comet",
			ui: {
				type: "select",
				options: orientedShapeOptions,
				label: "Shape",
				group: "Flock"
			}
		},
		count: {
			default: 2e3,
			description: "Number of flocking agents",
			ui: {
				type: "range",
				min: 100,
				max: 4e3,
				step: 100,
				label: "Count",
				group: "Flock"
			}
		},
		speed: {
			default: 2,
			description: "Overall cruising speed of the flock",
			ui: {
				type: "range",
				min: .2,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Flock"
			}
		},
		seed: {
			default: 0,
			description: "Re-rolls the flock's starting formation — each value spawns the murmuration in a different arrangement",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Flock"
			}
		},
		size: {
			default: 1.5,
			description: "Size of each agent",
			ui: {
				type: "range",
				min: .5,
				max: 3,
				step: .05,
				label: "Size",
				group: "Flock"
			}
		},
		trails: {
			default: 0,
			description: "How long each agent's motion trail persists — 0 draws crisp agents, 1 leaves long ribbons",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Trails",
				group: "Flock"
			}
		},
		separation: {
			default: 1.7,
			description: "How strongly agents steer away from crowding their neighbours",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Separation",
				group: "Flocking"
			}
		},
		alignment: {
			default: 1.5,
			description: "How strongly agents match their neighbours' heading — the driver of coherent murmuration ribbons",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Alignment",
				group: "Flocking"
			}
		},
		cohesion: {
			default: 1,
			description: "How strongly agents steer toward the local center of the flock",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Cohesion",
				group: "Flocking"
			}
		},
		perception: {
			default: .12,
			description: "How far each agent can sense its neighbours (in screen heights) — larger sees bigger, smoother flocks",
			ui: {
				type: "range",
				min: .05,
				max: .35,
				step: .01,
				label: "Perception",
				group: "Flocking"
			}
		},
		cursorMode: {
			default: "repel",
			description: "How the flock reacts to the cursor",
			ui: {
				type: "select",
				options: [
					{
						label: "None",
						value: "none"
					},
					{
						label: "Attract",
						value: "attract"
					},
					{
						label: "Predator",
						value: "repel"
					}
				],
				label: "Cursor",
				group: "Interaction"
			}
		},
		cursorStrength: {
			default: 1.5,
			description: "Strength of the cursor's pull or push on the flock",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Cursor Strength",
				group: "Interaction"
			}
		}
	},
	...agentSim({
		layout: simLayout,
		params: SimParams,
		maxAgents: MAX_AGENTS,
		countCap: {
			desktop: MAX_AGENTS,
			mobile: COUNT_MAX_MOBILE
		},
		output: {
			key: "outTex",
			name: "boidsTexture",
			size: [RES, RES],
			format: STATE_FORMAT
		},
		bake: ({ getCpuValue }) => {
			const render = boidsRender(getCpuValue("agentShape") || "arrow", getCpuValue("colorSpace") ?? 2);
			return {
				pipelines: {
					init: {
						kernel: boidsInitKernel,
						threads: "max"
					},
					update: {
						kernel: boidsUpdateKernel,
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
			return (frameParams) => {
				const { dt, aspect, pointerX, pointerY } = readAgentFrame(frameParams);
				const domainX = Math.max(aspect, .01);
				const count = sys.resolveCount(g("count", 2e3));
				const seed = g("seed", 0);
				const maxSpeed = BASE_SPEED * g("speed", 2);
				const maxForce = maxSpeed * FORCE_RATIO;
				const perception = g("perception", .12);
				const cursorModeStr = getCpuValue("cursorMode") ?? "repel";
				const cursorMode = cursorModeStr === "attract" ? 1 : cursorModeStr === "repel" ? 2 : 0;
				const cursorX = pointerX * aspect;
				const cursorY = pointerY;
				const cursorRadius = .22;
				const colA = getCpuValue("colorA");
				const colB = getCpuValue("colorB");
				const bodyR = Math.min(Math.max(g("size", 1.5), .5), 3) * 2 / SIZE_REF_RES;
				sys.writeParams({
					colA: vec4f(colA?.x ?? .56, colA?.y ?? .77, colA?.z ?? 1, colA?.w ?? 1),
					colB: vec4f(colB?.x ?? 1, colB?.y ?? .48, colB?.z ?? .85, colB?.w ?? 1),
					count,
					trails: Math.min(Math.max(g("trails", .35), 0), 1) * .92,
					dt,
					aspect,
					domainX,
					maxSpeed,
					maxForce,
					perceptionSq: perception * perception,
					sepRadiusSq: perception * .5 * (perception * .5),
					sepW: g("separation", 1.7),
					aliW: g("alignment", 1.5),
					cohW: g("cohesion", 1),
					cursorX,
					cursorY,
					cursorMode,
					cursorRadius,
					cursorRadiusSq: cursorRadius * cursorRadius,
					cursorForce: g("cursorStrength", 1.5) * maxSpeed * 12,
					bodyR,
					margin: .07,
					turnForce: maxSpeed * 2.2,
					seed
				});
				return sys.frame({
					count,
					reseed: seed
				});
			};
		},
		fragment: {
			output: "boidsTexture",
			fallback: "transparent"
		}
	})
});
var Boids_default = componentDefinition;
export { boidsUpdateKernel as a, makeBoidsSplatKernel as c, boidsSplatKernel as i, boidsInitKernel as n, componentDefinition as o, boidsResolveKernel as r, makeBoidsResolveKernel as s, Boids_default as t };
