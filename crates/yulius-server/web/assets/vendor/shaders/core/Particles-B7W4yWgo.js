import { $f as f32, Ac as defineStd, D as r3SubCellOffset, E as pointerToShapeLocal, Jf as textureStorage2d, Kf as atomic, Lu as resolveShapeType, M as force, N as integrator, Nu as sdf3d_exports, O as shapeField, Ru as shapeEffectBoundingBoxDeclaration, S as expDecay, Sl as resolveRenderRes, T as omegaFromRotationDeltas, Wf as vec4f, Xf as struct, Yf as arrayOf, _ as agentSim, bl as makeCpuValueGetter, ep as u32, gu as pointShapeOptions, mf as DEG_TO_RAD$1, qf as texture2d, rd as sdf_exports, uu as makeBallCloudInit, v as renderAgents, x as entrainmentFromOmega, xf as typegpu_default, xl as readAgentFrame, yl as SIZE_REF_RES } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var { buildAnalyticSdfFn, createSdfDataTexture, SVG_SDF_SIZE } = sdf_exports;
var { createAnalytic3dSdfSetup, createSvg3dSdfSetup, is3dShapeType, isSvg3dShapeType, resolveShapeSubProp, SHAPE3D_DEFAULTS, SVG3D_DEFAULTS, VOLUMETRIC_FIELD_FORMAT } = sdf3d_exports;
var DEG_TO_RAD = DEG_TO_RAD$1;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
function parseShapeConfig(raw) {
	if (raw && typeof raw === "object") return raw;
	if (typeof raw === "string") try {
		return JSON.parse(raw);
	} catch {
		return {};
	}
	return {};
}
var MAX_PARTICLES = 16e3;
var DGRID = 32;
var DCELLS = DGRID * DGRID * DGRID;
var DOMAIN = 1;
var OUT_RES = resolveRenderRes({
	desktop: 1024,
	mobile: 768
});
var SIZE_SCALE = OUT_RES / SIZE_REF_RES;
var EXTQ = 2.6;
var MAX_SPLAT_SIZE = 6;
var MAX_SPEED = 3;
var STATE_FORMAT = "rgba16float";
var SimParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	colA: vec4f,
	colB: vec4f,
	dt: f32,
	time: f32,
	spread: f32,
	agitation: f32,
	dragMul: f32,
	gravX: f32,
	gravY: f32,
	halfDepth: f32,
	cursorX: f32,
	cursorY: f32,
	cursorForce: f32,
	cursorRadSq: f32,
	saRadius: f32,
	saSides: f32,
	saRounding: f32,
	saInnerRatio: f32,
	saRotation: f32,
	saHeight: f32,
	saOffset: f32,
	saAperture: f32,
	centerX: f32,
	centerYv: f32,
	scale: f32,
	rotC: f32,
	rotS: f32,
	aspect: f32,
	size: f32,
	exposure: f32,
	softness: f32,
	speedColorK: f32,
	omegaX: f32,
	omegaY: f32,
	omegaZ: f32,
	entrain: f32,
	gridOffX: f32,
	gridOffY: f32,
	gridOffZ: f32
}), "SimParams");
var simLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	pos: {
		storage: arrayOf(vec4f, MAX_PARTICLES),
		access: "mutable"
	},
	vel: {
		storage: arrayOf(vec4f, MAX_PARTICLES),
		access: "mutable"
	},
	dens: {
		storage: arrayOf(atomic(u32), DCELLS),
		access: "mutable"
	},
	accumE: {
		storage: arrayOf(atomic(u32), OUT_RES * OUT_RES),
		access: "mutable"
	},
	accumS: {
		storage: arrayOf(atomic(u32), OUT_RES * OUT_RES),
		access: "mutable"
	},
	params: { uniform: SimParams },
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "simLayout");
var svgSdfLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({ sdfSource: { texture: texture2d(f32) } }), "svgSdfLayout");
const particleInitKernel = makeBallCloudInit(simLayout, "particlesInit");
var pressure = force.pressure(simLayout, {
	gridDim: DGRID,
	domain: DOMAIN,
	names: {
		clear: "particlesClearDensity",
		splat: "particlesDensity"
	}
});
const particleClearDensityKernel = pressure.clearKernel;
const particleDensityKernel = pressure.splatKernel;
function makeParticleUpdateKernel(evalSdf) {
	return integrator.forces3d(simLayout, {
		forces: [
			pressure.force,
			force.containment(simLayout, {
				field: evalSdf,
				gradEps: .02,
				wall: {
					featherFrom: -.1,
					featherTo: .01,
					base: 1.1,
					springK: 22
				},
				recall: {
					from: DOMAIN * .85,
					to: DOMAIN * 1.1,
					k: 9
				},
				homing: {
					from: .12,
					to: .45,
					k: 5
				},
				entrainment: { featherHalf: .05 }
			}),
			force.cursorXY(simLayout),
			force.gravity(simLayout),
			force.turbulence(simLayout, {
				posScale: 193,
				timeX: 31.7,
				timeY: 27.3,
				seedScale: 997,
				gain: 5
			})
		],
		maxSpeed: MAX_SPEED,
		posClamp: 1.9,
		name: "particlesUpdate"
	});
}
function particlesRender(shape, colorSpaceMode) {
	return renderAgents.volume(simLayout, {
		shape,
		outRes: OUT_RES,
		maxSplatSize: MAX_SPLAT_SIZE,
		extQ: EXTQ,
		ramp: {
			colorSpace: colorSpaceMode,
			exposure: true,
			trails: "none"
		},
		names: {
			splat: "particlesSplat",
			glow: "particlesGlowProfile",
			profile: "particlesProfile",
			resolve: "particlesResolve"
		},
		extraOnSplat: true
	});
}
const makeParticleSplatKernel = (shape) => particlesRender(shape, 0).splat.kernel;
const makeParticleResolveKernel = (mode) => particlesRender("dot", mode).resolve.kernel;
const particleSplatKernel = makeParticleSplatKernel("dot");
function resolveShapeField(params, shapeSdfUrl, shapeType) {
	const { gpu, getCpuValue, onCleanup } = params;
	const root = gpu.root;
	if (shapeSdfUrl && isSvg3dShapeType(shapeType)) {
		const setup3d = createSvg3dSdfSetup(params, shapeSdfUrl, () => getCpuValue("shape"));
		const dummyField = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(root.createTexture({
			size: [1, 1],
			format: VOLUMETRIC_FIELD_FORMAT
		}).$usage("storage"), "dummyField");
		onCleanup(() => dummyField.destroy());
		const extraBindGroup = root.createBindGroup(setup3d.layout, {
			field: dummyField,
			params: setup3d.marchParamsBuffer.buffer,
			sdfSource: setup3d.sdfSourceTexture
		});
		return {
			evalSdf: shapeField.svgLifted3d(setup3d.layout, setup3d.sdfFn),
			extraBindGroup,
			setup3d
		};
	}
	if (shapeSdfUrl) {
		const sdfTex = createSdfDataTexture(params, shapeSdfUrl);
		const extraBindGroup = root.createBindGroup(svgSdfLayout, { sdfSource: sdfTex.texture.texture });
		return {
			evalSdf: shapeField.svgExtruded(simLayout, svgSdfLayout, { size: SVG_SDF_SIZE }),
			extraBindGroup,
			setup3d: null
		};
	}
	if (is3dShapeType(shapeType)) {
		const setup3d = createAnalytic3dSdfSetup(root, shapeType, parseShapeConfig(getCpuValue("shape")), () => getCpuValue("shape"));
		const dummyField = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(root.createTexture({
			size: [1, 1],
			format: VOLUMETRIC_FIELD_FORMAT
		}).$usage("storage"), "dummyField");
		onCleanup(() => dummyField.destroy());
		const extraBindGroup = root.createBindGroup(setup3d.layout, {
			field: dummyField,
			params: setup3d.marchParamsBuffer.buffer
		});
		return {
			evalSdf: shapeField.analytic3d(setup3d.layout, setup3d.sdfFn),
			extraBindGroup,
			setup3d
		};
	}
	const sdf2d = buildAnalyticSdfFn(shapeType);
	return {
		evalSdf: shapeField.analyticExtruded(simLayout, sdf2d),
		extraBindGroup: null,
		setup3d: null
	};
}
const componentDefinition = defineStd({
	name: "Particles",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "A swarm of simulated particles that settles into the shape, filling it evenly — flat, SVG or true 3D volumes — scattering from the cursor (or chasing it), tumbling when the shape moves, and always drifting back home",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	props: {
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the shape relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the shape (1 = default size)",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .01,
				label: "Scale",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		colorA: {
			default: "#8ec5ff",
			transform: transformColor,
			description: "Color of particles at rest",
			ui: {
				type: "color",
				label: "Rest Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ff7ad9",
			transform: transformColor,
			description: "Color particles shift toward when agitated — scattering from the cursor or catching up to a moving shape",
			ui: {
				type: "color",
				label: "Excited Color",
				group: "Colors"
			}
		},
		count: {
			default: 4e3,
			description: "Number of simulated particles",
			ui: {
				type: "range",
				min: 1e3,
				max: MAX_PARTICLES,
				step: 1e3,
				label: "Count",
				group: "Particles"
			}
		},
		size: {
			default: 1.5,
			description: "Base particle size (each particle also varies slightly, and grows as it drifts toward the viewer)",
			ui: {
				type: "range",
				min: .6,
				max: 6,
				step: .1,
				label: "Size",
				group: "Particles"
			}
		},
		particleShape: {
			default: "dot",
			compileTime: true,
			description: "What each particle is drawn as — a crisp dot or square (softness still feathers them), or a soft glow puff",
			ui: {
				type: "select",
				options: pointShapeOptions,
				label: "Shape",
				group: "Particles"
			}
		},
		spread: {
			default: 1,
			description: "The pressure pushing particles apart — how strongly they insist on even spacing inside the shape",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .01,
				label: "Spread",
				group: "Physics"
			}
		},
		agitation: {
			default: .12,
			description: "Idle thermal motion of the swarm — 0 freezes it crystal-still once settled",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Agitation",
				group: "Physics"
			}
		},
		damping: {
			default: .4,
			description: "How quickly disturbed particles calm back down and settle into place",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Damping",
				group: "Physics"
			}
		},
		gravity: {
			default: 0,
			description: "Downward pull on the swarm — negative floats the particles upward",
			ui: {
				type: "range",
				min: -2,
				max: 2,
				step: .01,
				label: "Gravity",
				group: "Physics"
			}
		},
		mouseInfluence: {
			default: 2,
			description: "Strength of the cursor field. Positive scatters particles away; negative pulls the swarm toward the cursor",
			ui: {
				type: "range",
				min: -5,
				max: 5,
				step: .01,
				label: "Cursor Force",
				group: "Interaction"
			}
		},
		mouseRadius: {
			default: .2,
			description: "Reach of the cursor field, in shape-local units",
			ui: {
				type: "range",
				min: .05,
				max: .8,
				step: .01,
				label: "Cursor Radius",
				group: "Interaction"
			}
		},
		exposure: {
			default: 1,
			description: "Brightness of the additive particle glow",
			ui: {
				type: "range",
				min: .2,
				max: 3,
				step: .01,
				label: "Exposure",
				group: "Particles"
			}
		},
		softness: {
			default: .1,
			description: "Edge softness of each particle — 0 is a crisp disc, 1 a soft glow puff",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Particles"
			}
		},
		depth: {
			default: .18,
			description: "Thickness of the particle volume behind flat shapes (3D shapes use their own true depth)",
			ui: {
				type: "range",
				min: .02,
				max: .6,
				step: .01,
				label: "Depth",
				group: "Particles"
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
			default: DEFAULT_SHAPE_CONFIG,
			description: "Serialized shape configuration (JSON)",
			ui: {
				type: "shape",
				label: "Shape",
				group: "Shape"
			}
		},
		shapeSdfUrl: {
			default: "",
			compileTime: true,
			description: "URL to a pre-generated SDF .bin file"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type"
		}
	},
	...agentSim({
		layout: simLayout,
		params: SimParams,
		maxAgents: MAX_PARTICLES,
		countCap: {
			desktop: MAX_PARTICLES,
			mobile: 6e3
		},
		output: {
			key: "outTex",
			name: "particleTexture",
			size: [OUT_RES, OUT_RES],
			format: STATE_FORMAT
		},
		bake: (params) => {
			const { getCpuValue } = params;
			const shapeSdfUrl = getCpuValue("shapeSdfUrl") || "";
			const shapeType = resolveShapeType(getCpuValue("shapeType") || "", getCpuValue("shape"));
			const shapeField$1 = resolveShapeField(params, shapeSdfUrl, shapeType);
			const render = particlesRender(getCpuValue("particleShape") || "dot", getCpuValue("colorSpace") ?? 0);
			return {
				pipelines: {
					init: {
						kernel: particleInitKernel,
						threads: "max",
						extra: true
					},
					clearDensity: {
						kernel: pressure.clearKernel,
						threads: "fixed",
						size: [pressure.cells]
					},
					density: {
						kernel: pressure.splatKernel,
						threads: "agents",
						extra: true
					},
					update: {
						kernel: makeParticleUpdateKernel(shapeField$1.evalSdf),
						threads: "agents",
						extra: true
					},
					splat: render.splat,
					resolve: render.resolve
				},
				initStep: "init",
				program: [
					"clearDensity",
					"density",
					"update",
					"splat",
					"resolve"
				],
				extraBindGroup: shapeField$1.extraBindGroup,
				setup: {
					shapeField: shapeField$1,
					shapeSdfUrl,
					shapeType
				}
			};
		},
		frame: (sys, { getCpuValue }, setupRaw) => {
			const { shapeField: shapeField$1, shapeSdfUrl, shapeType } = setupRaw;
			const setup3d = shapeField$1.setup3d;
			let elapsed = 0;
			const springs = /* @__PURE__ */ new Map();
			const num = (v, f, frame, k) => resolveShapeSubProp(v, f, frame, k);
			let time = 0;
			let frameIdx = 0;
			let prevRot = null;
			const CELL = 2 * DOMAIN / DGRID;
			const g = makeCpuValueGetter(getCpuValue);
			return (frameParams) => {
				const { dt, aspect, pointerX, pointerY } = readAgentFrame(frameParams, 1);
				elapsed += dt;
				time += dt;
				const frame = {
					elapsed,
					deltaTime: dt,
					pointerX,
					pointerY,
					springs
				};
				const centerView = getCpuValue("center");
				const centerX = centerView?.x ?? .5;
				const centerYv = 1 - (centerView?.y ?? .5);
				const scale = Math.max(g("scale", 1), .01);
				const rotRad = g("rotation", 0) * DEG_TO_RAD;
				const rotC = Math.cos(rotRad);
				const rotS = Math.sin(rotRad);
				const cursor = pointerToShapeLocal({
					pointerX,
					pointerY,
					centerX,
					centerYv,
					scale,
					rotC,
					rotS,
					aspect
				});
				const gravMag = g("gravity", 0) * 1.2;
				const gravX = rotS * gravMag;
				const gravY = -(rotC * gravMag);
				const cfg = parseShapeConfig(getCpuValue("shape"));
				const isSvg3d = isSvg3dShapeType(shapeType);
				const halfDepth = shapeSdfUrl && isSvg3d ? num(cfg.depth, SVG3D_DEFAULTS.depth ?? .25, frame, "depth") * .5 : Math.max(g("depth", .18), .01) * .5;
				const saRotationV = num(cfg.rotation, 0, frame, "shapeRotation");
				let om = {
					x: 0,
					y: 0,
					z: 0
				};
				if (setup3d) {
					const d3 = isSvg3dShapeType(shapeType) ? SVG3D_DEFAULTS : SHAPE3D_DEFAULTS[shapeType] ?? {};
					const next = {
						x: num(cfg.rotX, d3.rotX ?? 0, frame, "rotX") * DEG_TO_RAD,
						y: num(cfg.rotY, d3.rotY ?? 0, frame, "rotY") * DEG_TO_RAD,
						z: num(cfg.rotZ, d3.rotZ ?? 0, frame, "rotZ") * DEG_TO_RAD
					};
					om = omegaFromRotationDeltas(prevRot, next, dt) ?? om;
					prevRot = next;
				} else if (!shapeSdfUrl) {
					const next = {
						x: 0,
						y: 0,
						z: saRotationV * DEG_TO_RAD
					};
					om = omegaFromRotationDeltas(prevRot, next, dt) ?? om;
					prevRot = next;
				}
				const { omegaX, omegaY, omegaZ, entrain } = entrainmentFromOmega(om, {
					cap: 10,
					gainRate: 2.5,
					gainMax: 5
				});
				const damping = g("damping", .4);
				const mouseRadius = g("mouseRadius", .22);
				const colA = getCpuValue("colorA");
				const colB = getCpuValue("colorB");
				const gridOff = r3SubCellOffset(frameIdx, CELL);
				sys.writeParams({
					colA: vec4f(colA?.x ?? 1, colA?.y ?? 1, colA?.z ?? 1, colA?.w ?? 1),
					colB: vec4f(colB?.x ?? 1, colB?.y ?? 1, colB?.z ?? 1, colB?.w ?? 1),
					dt,
					time,
					spread: g("spread", 1),
					agitation: g("agitation", .12),
					dragMul: expDecay(2.2 + damping * 6, dt),
					gravX,
					gravY,
					halfDepth,
					cursorX: cursor.x,
					cursorY: cursor.y,
					cursorForce: g("mouseInfluence", 1.2) * 3,
					cursorRadSq: mouseRadius * mouseRadius,
					saRadius: num(cfg.radius ?? cfg.width ?? cfg.bottomWidth, .35, frame, "radius"),
					saSides: num(cfg.sides, 6, frame, "sides"),
					saRounding: num(cfg.rounding, 0, frame, "rounding"),
					saInnerRatio: num(cfg.innerRatio ?? cfg.thickness ?? cfg.spread ?? cfg.topWidth ?? cfg.topRatio, .4, frame, "innerRatio"),
					saRotation: saRotationV,
					saHeight: num(cfg.height, .25, frame, "height"),
					saOffset: num(cfg.offset ?? cfg.skew, .2, frame, "offset"),
					saAperture: num(cfg.aperture, 270, frame, "aperture"),
					centerX,
					centerYv,
					scale,
					rotC,
					rotS,
					aspect,
					size: g("size", 2.2) * SIZE_SCALE,
					exposure: g("exposure", 1),
					softness: Math.min(Math.max(g("softness", .5), 0), 1),
					speedColorK: 2 / MAX_SPEED,
					omegaX,
					omegaY,
					omegaZ,
					entrain,
					gridOffX: gridOff.x,
					gridOffY: gridOff.y,
					gridOffZ: gridOff.z
				});
				frameIdx++;
				setup3d?.update({
					deltaTime: dt,
					pointer: {
						x: pointerX,
						y: pointerY
					}
				});
				return sys.frame({ count: sys.resolveCount(g("count", 4e3)) });
			};
		},
		fragment: {
			output: "particleTexture",
			fallback: "transparent"
		}
	})
});
var Particles_default = componentDefinition;
export { makeParticleUpdateKernel as a, particleInitKernel as c, makeParticleSplatKernel as i, particleSplatKernel as l, componentDefinition as n, particleClearDensityKernel as o, makeParticleResolveKernel as r, particleDensityKernel as s, Particles_default as t };
