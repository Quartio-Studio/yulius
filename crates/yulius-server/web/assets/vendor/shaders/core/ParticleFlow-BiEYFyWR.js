import { $f as f32, Ac as defineStd, Cs as recompileWhen, Jf as textureStorage2d, Kf as atomic, N as integrator, Pd as createStateBuffer, Sl as resolveRenderRes, Wf as vec4f, Xf as struct, Yf as arrayOf, _ as agentSim, _l as createStableFluidsPasses, ad as createIdleGate, cd as pathStampRibbon, ep as u32, gl as createFluidKernelPass, hl as buildVelocityImpulseKernel, hu as orientedShapeOptions, mu as makeUniformScatterInit, od as createPointerVelocityTracker, pl as buildStableFluidsKernels, qf as texture2d, v as renderAgents, xf as typegpu_default, yl as SIZE_REF_RES } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var N = 256;
var FLUID_COUNT = N * N;
var JACOBI_ITERS = resolveRenderRes({
	desktop: 10,
	mobile: 6
});
var VEL_FORMAT = "rgba16float";
var RES = resolveRenderRes({
	desktop: 1024,
	mobile: 512
});
var STATE_FORMAT = "rgba16float";
var MAX_PARTICLES = 16384;
var COUNT_MAX_MOBILE = 6e3;
var SPLAT_R = 14;
var IMPULSE_K = .16;
var MAX_STEPS = 16;
var FORCE_RADIUS_GRID = .16 * N;
var VEL_FADE_MAX = 4;
var VEL_FADE_MIN = .05;
var AMBIENT_K = 26;
var AMBIENT_FREQ = 2.4;
var AMBIENT_DRIFT = .09;
var INERTIA_RATE = 6;
var SPEED_NORM = .35;
var HOME_RATE = 1.8;
var WARMUP_FRAMES = 3;
var FlowParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	dt: f32,
	curlStrength: f32,
	velFade: f32,
	ambient: f32,
	ambientTime: f32,
	ambientFreq: f32
}), "FlowParams");
var SplatParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	posX: f32,
	posY: f32,
	velX: f32,
	velY: f32,
	radius: f32
}), "SplatParams");
var SimParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	colA: vec4f,
	colB: vec4f,
	dt: f32,
	aspect: f32,
	trails: f32,
	bodyR: f32,
	speedNorm: f32,
	advect: f32
}), "SimParams");
var fluidLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	velA: {
		storage: arrayOf(vec4f, FLUID_COUNT),
		access: "mutable"
	},
	velB: {
		storage: arrayOf(vec4f, FLUID_COUNT),
		access: "mutable"
	},
	pressure: {
		storage: arrayOf(f32, FLUID_COUNT),
		access: "mutable"
	},
	divergence: {
		storage: arrayOf(f32, FLUID_COUNT),
		access: "mutable"
	},
	params: { uniform: FlowParams },
	sparams: { uniform: SplatParams },
	velOutTex: { storageTexture: textureStorage2d(VEL_FORMAT, "write-only") }
}), "fluidLayout");
var pfLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	agents: {
		storage: arrayOf(vec4f, MAX_PARTICLES),
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
	velTex: { texture: texture2d(f32) },
	params: { uniform: SimParams },
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "pfLayout");
const particleFlowForceKernel = buildVelocityImpulseKernel(fluidLayout, {
	n: N,
	namePrefix: "particleFlow"
});
var solverKernels = buildStableFluidsKernels(fluidLayout, {
	n: N,
	namePrefix: "particleFlow",
	boundary: "clamped",
	dye: "none",
	publishVelocityTexture: true,
	ambientWind: { gain: AMBIENT_K }
});
const { curl: particleFlowCurlKernel, divergence: particleFlowDivergenceKernel, jacobi: particleFlowJacobiKernel, gradSubtract: particleFlowGradSubtractKernel, advectVel: particleFlowAdvectVelKernel, copyVel: particleFlowCopyVelKernel } = solverKernels;
const particleFlowVorticityKernel = solverKernels.vorticity;
const particleFlowInitKernel = makeUniformScatterInit(pfLayout, "particleFlowInit");
const particleFlowIntegrateKernel = integrator.advect2d(pfLayout, {
	gridN: N,
	inertiaRate: INERTIA_RATE,
	homeRate: HOME_RATE,
	name: "particleFlowIntegrate"
});
function particleFlowRender(shape, colorSpaceMode, trailsOn) {
	return renderAgents.orientedWorld(pfLayout, {
		shape,
		res: RES,
		splatRCap: SPLAT_R,
		heading: "velocity",
		agitation: "speed",
		comet: true,
		ramp: {
			colorSpace: colorSpaceMode,
			trails: { baked: trailsOn }
		},
		names: {
			splat: "particleFlowSplat",
			resolve: "particleFlowResolve"
		}
	});
}
const makeParticleFlowSplatKernel = (shape) => particleFlowRender(shape, 2, false).splat.kernel;
const makeParticleFlowResolveKernel = (mode, trailsOn) => particleFlowRender("streak", mode, trailsOn).resolve.kernel;
const particleFlowSplatKernel = makeParticleFlowSplatKernel("streak");
const particleFlowResolveKernel = makeParticleFlowResolveKernel(2, false);
const componentDefinition = defineStd({
	name: "ParticleFlow",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Thousands of drifting dust particles carried by a real incompressible fluid field the cursor stirs — drag to plow a wake and the particles ride the eddies and vortices it leaves behind, while a gentle ambient breeze keeps the whole scene breathing on its own",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	props: {
		colorA: {
			default: "#8ec5ff",
			transform: transformColor,
			description: "Color of particles drifting slowly with the flow",
			ui: {
				type: "color",
				label: "Rest Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffd98e",
			transform: transformColor,
			description: "Color particles flash toward as they speed up in the fast jets and wakes",
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
		shape: {
			default: "streak",
			compileTime: true,
			description: "What each particle is drawn as — a streak or arrow that stretches along the flow, a plain dot, a square, or a soft glowing comet",
			ui: {
				type: "select",
				options: orientedShapeOptions,
				label: "Shape",
				group: "Particles"
			}
		},
		count: {
			default: 6e3,
			description: "Number of drifting particles",
			ui: {
				type: "range",
				min: 1e3,
				max: 16e3,
				step: 500,
				label: "Count",
				group: "Particles"
			}
		},
		size: {
			default: 1.2,
			description: "Size of each particle",
			ui: {
				type: "range",
				min: .3,
				max: 3,
				step: .05,
				label: "Size",
				group: "Particles"
			}
		},
		trails: {
			default: 0,
			recompile: recompileWhen((previousValue, newValue) => previousValue > 0 !== newValue > 0),
			description: "How long each particle's motion trail persists — 0 draws crisp dust, 1 leaves long flowing ribbons",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Trails",
				group: "Particles"
			}
		},
		speed: {
			default: 1,
			description: "How strongly the fluid carries the particles — higher makes the dust ride the flow faster",
			ui: {
				type: "range",
				min: .2,
				max: 3,
				step: .05,
				label: "Flow Speed",
				group: "Particles"
			}
		},
		force: {
			default: 1,
			description: "How hard the cursor's motion pushes the fluid — higher plows faster, more violent wakes and jets",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .05,
				label: "Cursor Force",
				group: "Dynamics"
			}
		},
		swirl: {
			default: 25,
			description: "Vorticity confinement — swirl energy that spins the flow into persistent eddies and vortices the dust orbits",
			ui: {
				type: "range",
				min: 0,
				max: 60,
				step: 1,
				label: "Swirl",
				group: "Dynamics"
			}
		},
		momentum: {
			default: .6,
			description: "How long the fluid keeps flowing after you stop dragging — high momentum lets the eddies carry the dust on and on",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Momentum",
				group: "Dynamics"
			}
		},
		ambient: {
			default: .15,
			description: "A gentle, ever-evolving breeze that keeps the dust drifting even without the cursor — set to 0 for a field that only moves when you stir it",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Ambient",
				group: "Dynamics"
			}
		}
	},
	...agentSim({
		layout: pfLayout,
		params: SimParams,
		maxAgents: MAX_PARTICLES,
		countCap: {
			desktop: MAX_PARTICLES,
			mobile: COUNT_MAX_MOBILE
		},
		output: {
			key: "outTex",
			name: "flowTexture",
			size: [RES, RES],
			format: STATE_FORMAT
		},
		externalKeys: ["velTex"],
		bake: (params) => {
			const { gpu, getCpuValue, onCleanup } = params;
			const root = gpu.root;
			const velTex = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(root.createTexture({
				size: [N, N],
				format: VEL_FORMAT
			}).$usage("storage", "sampled"), "velTex");
			onCleanup(() => velTex.destroy());
			const paramsU = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(root.createUniform(FlowParams), "paramsU");
			const splatU = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(root.createUniform(SplatParams), "splatU");
			const fluidBg = root.createBindGroup(fluidLayout, {
				velA: createStateBuffer(root, vec4f, FLUID_COUNT),
				velB: createStateBuffer(root, vec4f, FLUID_COUNT),
				pressure: createStateBuffer(root, f32, FLUID_COUNT),
				divergence: createStateBuffer(root, f32, FLUID_COUNT),
				params: paramsU.buffer,
				sparams: splatU.buffer,
				velOutTex: velTex
			});
			const force = createFluidKernelPass(root, particleFlowForceKernel, {
				n: N,
				bindGroup: fluidBg
			});
			const vorticity = createFluidKernelPass(root, particleFlowVorticityKernel, {
				n: N,
				bindGroup: fluidBg
			});
			const solver = createStableFluidsPasses(root, solverKernels, {
				n: N,
				bindGroup: fluidBg
			});
			const trailsCpu = getCpuValue("trails");
			const render = particleFlowRender(getCpuValue("shape") || "streak", getCpuValue("colorSpace") ?? 2, typeof trailsCpu === "number" && trailsCpu > 0);
			return {
				pipelines: {
					init: {
						kernel: particleFlowInitKernel,
						threads: "max"
					},
					integrate: {
						kernel: particleFlowIntegrateKernel,
						threads: "agents"
					},
					splat: render.splat,
					resolve: render.resolve
				},
				program: [
					"integrate",
					"splat",
					"resolve"
				],
				bindNow: { velTex },
				setup: {
					force,
					vorticity,
					solver,
					paramsU,
					splatU
				}
			};
		},
		frame: (sys, { getCpuValue }, setupRaw) => {
			const { force, vorticity, solver, paramsU, splatU } = setupRaw;
			let lastTime = Date.now();
			let ambientTime = 0;
			let initialized = false;
			const pointer = createPointerVelocityTracker();
			const idle = createIdleGate({ warmupFrames: WARMUP_FRAMES + 1 });
			const num = (key, fallback) => {
				const v = getCpuValue(key);
				return typeof v === "number" ? v : fallback;
			};
			return (frameParams) => {
				const fp = frameParams;
				const now = Date.now();
				const dt = Math.min(fp.deltaTime ?? (now - lastTime) / 1e3, .033);
				lastTime = now;
				if (dt < .001) return null;
				const h = fp.dimensions?.height ?? 0;
				const aspect = h > 0 ? (fp.dimensions?.width ?? 0) / h : 16 / 9;
				const domainX = Math.max(aspect, .01);
				const forceVal = num("force", 1);
				const swirl = num("swirl", 25);
				const ambient = Math.min(Math.max(num("ambient", .15), 0), 1);
				const momentum = Math.min(Math.max(num("momentum", .6), 0), 1);
				const advect = num("speed", 1);
				const count = sys.resolveCount(num("count", 6e3), 100);
				const bodyR = Math.min(Math.max(num("size", 1.2), .3), 3) * 1.3 / SIZE_REF_RES;
				const ptr = pointer.update(fp.pointer, dt);
				const active = ptr.moving || ambient > .001;
				if (active) idle.markActive();
				ambientTime += dt * AMBIENT_DRIFT;
				if (initialized && idle.shouldSkip(2 + momentum * 13, active)) return null;
				idle.tickFrame(dt);
				const velFade = VEL_FADE_MAX * Math.pow(VEL_FADE_MIN / VEL_FADE_MAX, momentum);
				paramsU.write({
					dt,
					curlStrength: swirl,
					velFade,
					ambient,
					ambientTime,
					ambientFreq: AMBIENT_FREQ
				});
				const colA = getCpuValue("colorA");
				const colB = getCpuValue("colorB");
				sys.writeParams({
					colA: vec4f(colA?.x ?? .56, colA?.y ?? .77, colA?.z ?? 1, colA?.w ?? 1),
					colB: vec4f(colB?.x ?? 1, colB?.y ?? .85, colB?.z ?? .56, colB?.w ?? 1),
					dt,
					aspect: domainX,
					trails: Math.min(Math.max(num("trails", 0), 0), 1) * .92,
					bodyR,
					speedNorm: SPEED_NORM,
					advect
				});
				const nodes = [];
				if (!initialized) {
					initialized = true;
					nodes.push(() => sys.pipelines.init.dispatchThreads(MAX_PARTICLES));
				}
				if (ptr.moving) {
					const velX = ptr.velX * N * forceVal * IMPULSE_K;
					const velY = ptr.velY * N * forceVal * IMPULSE_K;
					pathStampRibbon(nodes, {
						fromX: ptr.prevX,
						fromY: ptr.prevY,
						dx: ptr.dx,
						dy: ptr.dy,
						dragDist: ptr.dragDist,
						stepSize: Math.max(.004, FORCE_RADIUS_GRID / N * .6),
						maxSteps: MAX_STEPS,
						scale: N,
						write: (posX, posY) => splatU.write({
							posX,
							posY,
							velX,
							velY,
							radius: FORCE_RADIUS_GRID
						}),
						pass: force
					});
				}
				nodes.push(...solver.solveSteps({
					jacobiIters: JACOBI_ITERS,
					vorticity
				}));
				nodes.push(...sys.frame({ count }) ?? []);
				return nodes;
			};
		},
		fragment: {
			output: "flowTexture",
			fallback: "transparent"
		}
	})
});
var ParticleFlow_default = componentDefinition;
export { particleFlowAdvectVelKernel as a, particleFlowDivergenceKernel as c, particleFlowInitKernel as d, particleFlowIntegrateKernel as f, particleFlowVorticityKernel as g, particleFlowSplatKernel as h, makeParticleFlowSplatKernel as i, particleFlowForceKernel as l, particleFlowResolveKernel as m, componentDefinition as n, particleFlowCopyVelKernel as o, particleFlowJacobiKernel as p, makeParticleFlowResolveKernel as r, particleFlowCurlKernel as s, ParticleFlow_default as t, particleFlowGradSubtractKernel as u };
