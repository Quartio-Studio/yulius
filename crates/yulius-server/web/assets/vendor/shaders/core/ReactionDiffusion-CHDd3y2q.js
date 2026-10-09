import { Ac as defineStd, Bc as buildGrayScottStep, Bd as ZERO, Dc as vec3, Ec as vec2, Gc as createGridKernelPass, Gs as clamp, Hd as call, Hf as vec2f, Js as dot, Ks as cos, Nd as createPingPongPair, Oc as vec4, Pd as createStateBuffer, Sc as splat3, Tc as sub, Uc as buildPairPublishKernel, Wc as buildScatterSeedKernel, Ws as add, af as colorMixing_exports, bc as smoothstep, cc as max, dc as mul, et as gridSim, fc as neg, it as op, lc as min, mc as pow, mf as DEG_TO_RAD, nc as gt, oc as local, od as createPointerVelocityTracker, pc as normalize, qs as div, uc as mix, ud as createLateBoundChildInput, vc as select, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, i as transformBoolean, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var cm = colorMixing_exports;
var N = 512;
var COUNT = N * N;
var STATE_FORMAT = "rgba16float";
var MAX_ITERATIONS = 16;
var SEED_DENSITY = .06;
var RELIEF_EPS = 1.5 / N;
var RELIEF_STRENGTH = 6;
var RELIEF_AMBIENT = .42;
var RELIEF_LIGHT_XY = .85;
var RELIEF_LIGHT_Z = .55;
var RELIEF_SHININESS = 18;
var RELIEF_SPEC = .35;
var DEG2RAD = DEG_TO_RAD;
var PRESETS = {
	coral: {
		feed: .0545,
		kill: .062
	},
	mitosis: {
		feed: .0367,
		kill: .0649
	},
	spots: {
		feed: .03,
		kill: .062
	},
	maze: {
		feed: .029,
		kill: .057
	},
	worms: {
		feed: .078,
		kill: .061
	},
	fingerprints: {
		feed: .037,
		kill: .06
	},
	holes: {
		feed: .039,
		kill: .058
	},
	solitons: {
		feed: .074,
		kill: .064
	},
	bubbles: {
		feed: .098,
		kill: .0555
	},
	waves: {
		feed: .014,
		kill: .045
	},
	flowers: {
		feed: .062,
		kill: .06093
	}
};
var CHILD_CONTRAST_MAX = 8;
var CHILD_DFEED = .022;
var CHILD_DKILL = .014;
var CHILD_FEED_MIN = .008;
var CHILD_FEED_MAX = .11;
var CHILD_KILL_MIN = .03;
var CHILD_KILL_MAX = .075;
var grayScott = buildGrayScottStep({
	n: N,
	namePrefix: "reactionDiffusion"
});
var grayScottChild = buildGrayScottStep({
	n: N,
	namePrefix: "reactionDiffusion",
	drive: {
		contrastMax: CHILD_CONTRAST_MAX,
		dFeed: CHILD_DFEED,
		dKill: CHILD_DKILL,
		feedMin: CHILD_FEED_MIN,
		feedMax: CHILD_FEED_MAX,
		killMin: CHILD_KILL_MIN,
		killMax: CHILD_KILL_MAX
	}
});
var seedSet = buildScatterSeedKernel({
	n: N,
	density: SEED_DENSITY,
	namePrefix: "reactionDiffusion"
});
var outputSet = buildPairPublishKernel({
	n: N,
	format: STATE_FORMAT,
	namePrefix: "reactionDiffusion"
});
const reactionKernel = grayScott.kernel;
const reactionKernelChild = grayScottChild.kernel;
const seedKernel = seedSet.kernel;
const outputKernel = outputSet.kernel;
const componentDefinition = defineStd({
	name: "ReactionDiffusion",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "A living Gray-Scott reaction-diffusion pattern that fills the layer and blooms wherever you drag the cursor",
	requiresRTT: false,
	requiresChild: false,
	acceptsOptionalChild: true,
	usesPointer: true,
	props: {
		preset: {
			default: "coral",
			description: "Named Gray-Scott regime — drives the feed & kill rates for a whole family of patterns. Choose Custom to dial feed & kill by hand",
			ui: {
				type: "select",
				options: [
					{
						label: "Coral",
						value: "coral"
					},
					{
						label: "Mitosis",
						value: "mitosis"
					},
					{
						label: "Spots",
						value: "spots"
					},
					{
						label: "Maze",
						value: "maze"
					},
					{
						label: "Worms",
						value: "worms"
					},
					{
						label: "Fingerprints",
						value: "fingerprints"
					},
					{
						label: "Holes",
						value: "holes"
					},
					{
						label: "Solitons",
						value: "solitons"
					},
					{
						label: "Bubbles",
						value: "bubbles"
					},
					{
						label: "Waves",
						value: "waves"
					},
					{
						label: "Flowers",
						value: "flowers"
					},
					{
						label: "Custom",
						value: "custom"
					}
				],
				label: "Pattern",
				group: "Pattern"
			}
		},
		feed: {
			default: .0545,
			description: "Feed rate — how fast chemical U is replenished (used when Pattern = Custom)",
			ui: {
				type: "range",
				min: .01,
				max: .1,
				step: .001,
				label: "Feed",
				group: "Pattern",
				condition: { preset: "custom" }
			}
		},
		kill: {
			default: .062,
			description: "Kill rate — how fast chemical V is removed (used when Pattern = Custom)",
			ui: {
				type: "range",
				min: .03,
				max: .07,
				step: .001,
				label: "Kill",
				group: "Pattern",
				condition: { preset: "custom" }
			}
		},
		diffusionRatio: {
			default: .5,
			description: "Ratio of the two chemicals' diffusion rates — shifts the pattern character (lower = tighter, more filament-like)",
			ui: {
				type: "range",
				min: .2,
				max: .9,
				step: .01,
				label: "Diffusion Ratio",
				group: "Pattern"
			}
		},
		featureSize: {
			default: 3,
			description: "Size of the pattern features — lower shows finer, smaller cells; higher magnifies them into larger cells",
			ui: {
				type: "range",
				min: 1,
				max: 6,
				step: .1,
				label: "Feature Size",
				group: "Pattern"
			}
		},
		speed: {
			default: 6,
			description: "Simulation steps per frame — higher evolves the pattern faster",
			ui: {
				type: "range",
				min: 1,
				max: MAX_ITERATIONS,
				step: 1,
				label: "Speed",
				group: "Pattern"
			}
		},
		brushSize: {
			default: .04,
			description: "Radius of the cursor brush that seeds new reagent as you drag",
			ui: {
				type: "range",
				min: .01,
				max: .3,
				step: .01,
				label: "Brush Size",
				group: "Interaction"
			}
		},
		brushStrength: {
			default: .6,
			description: "How much reagent the cursor injects while dragging",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Brush Strength",
				group: "Interaction"
			}
		},
		colorA: {
			default: "transparent",
			transform: transformColor,
			description: "Background color (empty regions between cells)",
			ui: {
				type: "color",
				label: "Background",
				group: "Colors"
			}
		},
		colorC: {
			default: "#38bdf8",
			transform: transformColor,
			description: "Accent color at the cell walls (mid pattern values)",
			ui: {
				type: "color",
				label: "Wall Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#e2f5ff",
			transform: transformColor,
			description: "Color of the dense cell interiors (high pattern values)",
			ui: {
				type: "color",
				label: "Cell Color",
				group: "Colors"
			}
		},
		contrast: {
			default: .5,
			description: "Sharpness of the cell walls — higher gives crisper, more graphic edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Contrast",
				group: "Colors"
			}
		},
		threshold: {
			default: .28,
			description: "Pattern level the colors are centered on — shifts the balance of background vs. cells",
			ui: {
				type: ["range", "map"],
				min: .05,
				max: .6,
				step: .01,
				label: "Threshold",
				group: "Colors"
			}
		},
		colorSpace: {
			default: "oklch",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		relief: {
			default: .4,
			description: "Treats the pattern as a raised surface and shades it with a light — higher gives a more sculpted, three-dimensional look (0 for a flat, graphic look)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Relief",
				group: "Lighting"
			}
		},
		lightAngle: {
			default: 135,
			description: "Direction the surface light comes from, in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Lighting"
			}
		},
		childInfluence: {
			default: .6,
			description: "When a child layer is nested, how strongly its brightness biases the pattern — the preset sets the character, the child steers where it grows dense vs. sparse (0 ignores the child)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Influence",
				group: "Child"
			}
		},
		childContrast: {
			default: .5,
			description: "Contrast applied to the child's brightness before it drives the pattern — higher pushes toward a crisp two-tone split (edges), lower keeps smooth tonal shading",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Contrast",
				group: "Child"
			}
		},
		childThreshold: {
			default: .5,
			description: "Brightness level of the child that reads as neutral — shifts which tones grow the pattern vs. clear it",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Threshold",
				group: "Child"
			}
		},
		childInvert: {
			default: false,
			transform: transformBoolean,
			description: "Flip which tones of the child grow the pattern (bright vs. dark areas)",
			ui: {
				type: "checkbox",
				label: "Invert",
				group: "Child"
			}
		}
	},
	...gridSim((params, root) => {
		const { getCpuValue, registerComputeTexture, onCleanup, childNode } = params;
		const hasChild = !!childNode;
		const bufferA = createStateBuffer(root, vec2f, COUNT);
		const bufferB = createStateBuffer(root, vec2f, COUNT);
		const outTex = root.createTexture({
			size: [N, N],
			format: STATE_FORMAT
		}).$usage("storage", "sampled");
		onCleanup(() => outTex.destroy());
		const rdTexture = registerComputeTexture(outTex);
		const variant = hasChild ? grayScottChild : grayScott;
		const paramsU = root.createUniform(variant.Params);
		const size = [N, N];
		const state = createPingPongPair(bufferA, bufferB);
		const output = createGridKernelPass(root, outputSet.kernel, { size });
		const outGroup = state.perSide((srcBuf) => output.with(root.createBindGroup(outputSet.layout, {
			srcBuf,
			outTex
		})));
		const seed = createGridKernelPass(root, seedSet.kernel, {
			size,
			bindGroup: root.createBindGroup(seedSet.layout, { stateBuf: bufferA })
		});
		const reaction = createGridKernelPass(root, variant.kernel, { size });
		const generatorGroup = hasChild ? null : state.groups((readBuf, writeBuf) => reaction.with(root.createBindGroup(grayScott.layout, {
			readBuf,
			writeBuf,
			params: paramsU.buffer
		})));
		const child = hasChild ? createLateBoundChildInput(params, (childTex) => state.groups((readBuf, writeBuf) => reaction.with(root.createBindGroup(grayScottChild.layout, {
			readBuf,
			writeBuf,
			params: paramsU.buffer,
			driveTex: childTex
		})))) : null;
		let needsSeed = true;
		let lastPreset = "";
		const pointer = createPointerVelocityTracker();
		let reactGroup = null;
		let iters = 1;
		return {
			outputs: { rdTexture },
			...child ? { bindInputs: child.bindInputs } : {},
			clampDt: .05,
			stages: [
				op.readyWhen(() => {
					reactGroup = child ? child.bound() : generatorGroup;
					return !!reactGroup;
				}),
				op.values("feed/kill+brush", (f) => {
					const fp = f.frameParams;
					const g = f.num;
					const hgt = fp.dimensions?.height ?? 0;
					const aspect = hgt > 0 ? (fp.dimensions?.width ?? 0) / hgt : 1;
					const preset = getCpuValue("preset") || "coral";
					if (preset !== lastPreset) {
						lastPreset = preset;
						needsSeed = true;
					}
					const pk = PRESETS[preset];
					const feed = pk ? pk.feed : g("feed", .055);
					const kill = pk ? pk.kill : g("kill", .062);
					const view = 1 / Math.max(g("featureSize", 3), 1e-4);
					const sx = Math.min(aspect, 1);
					const sy = Math.min(1 / aspect, 1);
					const move = pointer.update(fp.pointer, .016);
					const fieldX = (move.x - .5) * sx * view + .5;
					const fieldY = (move.y - .5) * sy * view + .5;
					const cursorSpeed = Math.sqrt(move.dx * move.dx + move.dy * move.dy);
					const velScale = cursorSpeed > 5e-4 && g("brushStrength", .6) > 0 ? Math.min(cursorSpeed / .02, 1) : 0;
					const brushRadGrid = g("brushSize", .04) * N * view;
					const invRaw = getCpuValue("childInvert");
					iters = Math.min(MAX_ITERATIONS, Math.max(1, Math.round(g("speed", 6))));
					paramsU.write({
						feed,
						kill,
						dv: g("diffusionRatio", .5),
						brushRadSq: brushRadGrid * brushRadGrid,
						brushStrength: g("brushStrength", .6),
						cursorX: fieldX * N,
						cursorY: fieldY * N,
						cursorActive: velScale,
						driveInfluence: g("childInfluence", .6),
						driveContrast: g("childContrast", .5),
						driveThreshold: g("childThreshold", .5),
						driveInvert: invRaw === true || invRaw === 1 ? 1 : 0,
						driveMapX: 1 / (sx * view),
						driveMapY: 1 / (sy * view),
						_pad0: 0,
						_pad1: 0
					});
				}),
				op.seedOnce({
					pass: seed,
					staleWhen: () => needsSeed,
					onSeed: () => {
						needsSeed = false;
						state.reset();
					}
				}),
				op.iterate({
					count: () => iters,
					step: (_i, nodes) => {
						nodes.push(reactGroup());
						state.swap();
					}
				}),
				op.publish(() => outGroup.read())
			]
		};
	}),
	gpu: { fragment: (params) => {
		const { ctx, computeOutputs, propValues, uniforms } = params;
		const rdField = computeOutputs?.rdTexture;
		if (!rdField) return ZERO;
		const view = local(div(1, uniforms.featureSize), "view");
		const sx = min(ctx.aspect, 1);
		const sy = min(div(1, ctx.aspect), 1);
		const sampleUV = local(vec2(add(mul(mul(sub(ctx.uv.member("x"), .5), sx), view), .5), add(mul(mul(sub(ctx.uv.member("y"), .5), sy), view), .5)), "sampleUV");
		const V = rdField.sample(sampleUV, "linearClamp").member("y");
		const w = local(max(sub(.6, mul(uniforms.contrast, .55)), .02), "rampW");
		const t = local(smoothstep(sub(uniforms.threshold, w), add(uniforms.threshold, w), V), "rampT");
		const colorSpaceMode = propValues.colorSpace ?? 0;
		const variant = cm.mixColorsVariants[colorSpaceMode] ?? cm.mixColorsLinear;
		const lowSeg = call(variant, "mixColors", [
			uniforms.colorA,
			uniforms.colorC,
			clamp(mul(t, 2), 0, 1)
		]);
		const highSeg = call(variant, "mixColors", [
			uniforms.colorC,
			uniforms.colorB,
			clamp(sub(mul(t, 2), 1), 0, 1)
		]);
		const color = local(select(gt(t, .5), highSeg, lowSeg), "rampColor");
		const vL = rdField.sample(add(sampleUV, vec2(-RELIEF_EPS, 0)), "linearClamp").member("y");
		const vR = rdField.sample(add(sampleUV, vec2(RELIEF_EPS, 0)), "linearClamp").member("y");
		const vT = rdField.sample(add(sampleUV, vec2(0, -RELIEF_EPS)), "linearClamp").member("y");
		const vB = rdField.sample(add(sampleUV, vec2(0, RELIEF_EPS)), "linearClamp").member("y");
		const gx = mul(sub(vR, vL), RELIEF_STRENGTH);
		const gy = mul(sub(vB, vT), RELIEF_STRENGTH);
		const n = local(normalize(vec3(neg(gx), neg(gy), 1)), "reliefN");
		const ang = local(mul(uniforms.lightAngle, DEG2RAD), "lightAng");
		const lightDir = local(normalize(vec3(mul(cos(ang), RELIEF_LIGHT_XY), mul(sin(ang), RELIEF_LIGHT_XY), RELIEF_LIGHT_Z)), "lightDir");
		const diff = clamp(dot(n, lightDir), 0, 1);
		const shade = add(RELIEF_AMBIENT, mul(1 - RELIEF_AMBIENT, diff));
		const spec = mul(pow(clamp(dot(n, normalize(add(lightDir, vec3(0, 0, 1)))), 0, 1), RELIEF_SHININESS), RELIEF_SPEC);
		const litRgb = add(mul(color.member("rgb"), shade), splat3(spec));
		return vec4(mix(color.member("rgb"), litRgb, uniforms.relief), color.member("a"));
	} }
});
var ReactionDiffusion_default = componentDefinition;
export { reactionKernelChild as a, reactionKernel as i, componentDefinition as n, seedKernel as o, outputKernel as r, ReactionDiffusion_default as t };
