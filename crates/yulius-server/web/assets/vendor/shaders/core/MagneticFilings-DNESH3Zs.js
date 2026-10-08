import { $f as f32, Ac as defineStd, Jf as textureStorage2d, Kf as atomic, N as integrator, P as torque, Sl as resolveRenderRes, Wf as vec4f, Xf as struct, Yf as arrayOf, _ as agentSim, b as createMotionAxis, bl as makeCpuValueGetter, ep as u32, fu as makeRestingDirectorInit, hu as orientedShapeOptions, j as field, v as renderAgents, w as fitJitteredGrid, xf as typegpu_default, xl as readAgentFrame, yl as SIZE_REF_RES } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var MAX_FILINGS = 12288;
var RES = resolveRenderRes({
	desktop: 1024,
	mobile: 768
});
var STATE_FORMAT = "rgba16float";
var SPLAT_R = 14;
var ALIGN_BASE = 45;
var REST_K = 6;
var DAMP_MIN = 3;
var DAMP_SPAN = 9;
var OMEGA_REF = 6;
var AGIT_COOL = 1;
var HOME_K = 2.5;
var PULL_K = .25;
var MAX_OFFSET = .12;
var OVERSCAN = .14;
var SimParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	colA: vec4f,
	colB: vec4f,
	count: f32,
	dt: f32,
	aspect: f32,
	domainX: f32,
	cursorX: f32,
	cursorY: f32,
	axisX: f32,
	axisY: f32,
	fieldType: f32,
	restMode: f32,
	strength: f32,
	reachSq: f32,
	alignK: f32,
	damping: f32,
	restK: f32,
	pullK: f32,
	homeK: f32,
	omegaRef: f32,
	agitCool: f32,
	gridCols: f32,
	cellW: f32,
	cellH: f32,
	jitter: f32,
	bodyR: f32
}), "SimParams");
var simLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	agents: {
		storage: arrayOf(vec4f, MAX_FILINGS),
		access: "mutable"
	},
	agit: {
		storage: arrayOf(f32, MAX_FILINGS),
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
	params: { uniform: SimParams },
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "simLayout");
const filingsInitKernel = makeRestingDirectorInit(simLayout, "filingsInit");
var filingsHome = field.jitteredGridHome(simLayout, { overscan: OVERSCAN });
var filingsField = field.dipoleOrRadial(simLayout);
const filingsUpdateKernel = integrator.orientation2d(simLayout, {
	home: filingsHome,
	fieldAt: filingsField,
	torques: [torque.alignToField(simLayout), torque.rest(simLayout)],
	maxOffset: MAX_OFFSET,
	name: "filingsUpdate"
});
function filingsRender(shape, colorSpaceMode) {
	return renderAgents.orientedWorld(simLayout, {
		shape,
		res: RES,
		splatRCap: SPLAT_R,
		heading: "angle",
		agitation: "buffer",
		comet: false,
		home: filingsHome,
		ramp: {
			colorSpace: colorSpaceMode,
			trails: "none"
		},
		names: {
			splat: "filingsSplat",
			resolve: "filingsResolve"
		}
	});
}
const makeFilingsSplatKernel = (shape) => filingsRender(shape, 2).splat.kernel;
const makeFilingsResolveKernel = (mode) => filingsRender("streak", mode).resolve.kernel;
const filingsSplatKernel = makeFilingsSplatKernel("streak");
const filingsResolveKernel = makeFilingsResolveKernel(2);
const componentDefinition = defineStd({
	name: "MagneticFilings",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Thousands of tiny iron filings scattered on paper that swing to align with a magnetic field around the cursor, tracing out the field lines — a dipole whose axis follows the cursor's motion draws the classic two-lobed swirl, and every sweep of the magnet sends a glowing wave of filings flipping and settling",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	props: {
		colorA: {
			default: "#be1ef9",
			transform: transformColor,
			description: "Color of filings resting quietly, aligned and still",
			ui: {
				type: "color",
				label: "Rest Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#6326ff",
			transform: transformColor,
			description: "Color filings flash toward as they swing and settle — a wave of it follows the moving magnet",
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
			default: "dot",
			compileTime: true,
			description: "How each filing is drawn — a slim streak or arrow reads as a compass needle, a box as a chunky filing, a dot or soft glow for a finer grain",
			ui: {
				type: "select",
				options: orientedShapeOptions,
				label: "Shape",
				group: "Filings"
			}
		},
		count: {
			default: 8e3,
			description: "Number of iron filings",
			ui: {
				type: "range",
				min: 500,
				max: 12e3,
				step: 500,
				label: "Count",
				group: "Filings"
			}
		},
		size: {
			default: 1,
			description: "Size of each filing",
			ui: {
				type: "range",
				min: .5,
				max: 3,
				step: .05,
				label: "Size",
				group: "Filings"
			}
		},
		restOrientation: {
			default: "random",
			description: "Which way filings point where the field can't reach them — randomly, all horizontal, or all vertical",
			ui: {
				type: "select",
				options: [
					{
						label: "Random",
						value: "random"
					},
					{
						label: "Horizontal",
						value: "horizontal"
					},
					{
						label: "Vertical",
						value: "vertical"
					}
				],
				label: "Rest Orientation",
				group: "Filings"
			}
		},
		fieldType: {
			default: "dipole",
			description: "The magnetic field the filings reveal — a dipole (bar-magnet lobes that follow the cursor's motion) or a radial monopole (spokes out from the cursor)",
			ui: {
				type: "select",
				options: [{
					label: "Dipole",
					value: "dipole"
				}, {
					label: "Radial",
					value: "radial"
				}],
				label: "Field Type",
				group: "Field"
			}
		},
		strength: {
			default: 1.7,
			description: "How strongly the magnet grips the filings — higher snaps more of them into sharp alignment",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Strength",
				group: "Field"
			}
		},
		reach: {
			default: .6,
			description: "How far the magnet's influence spreads (in screen heights) — larger reveals more of the field at once",
			ui: {
				type: "range",
				min: .1,
				max: 1,
				step: .01,
				label: "Reach",
				group: "Field"
			}
		},
		response: {
			default: .5,
			description: "How briskly filings swing and settle — low is loose and floppy with a long wobble, high snaps into place quickly",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Response",
				group: "Field"
			}
		}
	},
	...agentSim({
		layout: simLayout,
		params: SimParams,
		maxAgents: MAX_FILINGS,
		countCap: {
			desktop: MAX_FILINGS,
			mobile: 6e3
		},
		output: {
			key: "outTex",
			name: "filingsTexture",
			size: [RES, RES],
			format: STATE_FORMAT
		},
		bake: ({ getCpuValue }) => {
			const render = filingsRender(getCpuValue("shape") || "streak", getCpuValue("colorSpace") ?? 2);
			return {
				pipelines: {
					init: {
						kernel: filingsInitKernel,
						threads: "max"
					},
					update: {
						kernel: filingsUpdateKernel,
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
			const motionAxis = createMotionAxis({
				smoothing: .1,
				teleport: .25
			});
			return (frameParams) => {
				const { dt, aspect, pointerX, pointerY } = readAgentFrame(frameParams);
				const domainX = Math.max(aspect, .01);
				const count = sys.resolveCount(g("count", 5e3));
				const { cols, cellW, cellH } = fitJitteredGrid(count, domainX, OVERSCAN);
				const cursorX = pointerX * aspect;
				const cursorY = pointerY;
				const axis = motionAxis.update(cursorX, cursorY);
				const fieldType = (getCpuValue("fieldType") ?? "dipole") === "radial" ? 1 : 0;
				const restStr = getCpuValue("restOrientation") ?? "random";
				const restMode = restStr === "horizontal" ? 1 : restStr === "vertical" ? 2 : 0;
				const strength = g("strength", 1);
				const reach = g("reach", .35);
				const response = Math.min(Math.max(g("response", .5), 0), 1);
				const colA = getCpuValue("colorA");
				const colB = getCpuValue("colorB");
				const bodyR = Math.min(Math.max(g("size", 1), .5), 3) * 1.7 / SIZE_REF_RES;
				sys.writeParams({
					colA: vec4f(colA?.x ?? .522, colA?.y ?? .573, colA?.z ?? .639, colA?.w ?? 1),
					colB: vec4f(colB?.x ?? 1, colB?.y ?? .616, colB?.z ?? .361, colB?.w ?? 1),
					count,
					dt,
					aspect,
					domainX,
					cursorX,
					cursorY,
					axisX: axis.x,
					axisY: axis.y,
					fieldType,
					restMode,
					strength,
					reachSq: reach * reach,
					alignK: ALIGN_BASE,
					damping: DAMP_MIN + response * DAMP_SPAN,
					restK: REST_K,
					pullK: PULL_K,
					homeK: HOME_K,
					omegaRef: OMEGA_REF,
					agitCool: AGIT_COOL,
					gridCols: cols,
					cellW,
					cellH,
					jitter: .85,
					bodyR
				});
				return sys.frame({ count });
			};
		},
		fragment: {
			output: "filingsTexture",
			fallback: "transparent"
		}
	})
});
var MagneticFilings_default = componentDefinition;
export { filingsSplatKernel as a, makeFilingsSplatKernel as c, filingsResolveKernel as i, componentDefinition as n, filingsUpdateKernel as o, filingsInitKernel as r, makeFilingsResolveKernel as s, MagneticFilings_default as t };
