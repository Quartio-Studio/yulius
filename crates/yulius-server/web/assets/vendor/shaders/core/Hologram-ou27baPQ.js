import { $s as fract, Ac as defineStd, Bd as ZERO, Dc as vec3, Ec as vec2, Gs as clamp, Ic as animatedTime, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Si as surfaceNoise, Tc as sub, Us as abs, Ws as add, Xr as hashNoise, Yr as guarded, Ys as exp, Zr as insideShape, Zs as float, bc as smoothstep, cc as max, dc as mul, gf as TWO_PI$1, hi as shapedSurface, lc as min, mc as pow, oc as local, pc as normalize, qs as div, uc as mix, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var TWO_PI = TWO_PI$1;
const componentDefinition = defineStd({
	name: "Hologram",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Volumetric sci-fi hologram — a translucent emissive projection of the shape with fresnel-lit edges, depth scan-lines that wrap 3D forms, CRT scanlines, flicker and beam wobble",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		chord: "firstLobe",
		stencil: {
			kind: "central",
			eps: .008
		},
		placeUV: (params) => {
			const { uniforms: u, ctx } = params;
			const t = animatedTime(params);
			const uvY = ctx.uv.member("y");
			const wob = add(mul(sin(add(mul(uvY, 38), mul(t, 2.3))), .5), mul(sin(sub(mul(uvY, 11.5), mul(t, 1.7))), .5));
			return vec2(add(ctx.uv.member("x"), mul(mul(wob, u.distortion), .012)), uvY);
		},
		surface: (frame, params) => {
			const { uniforms: u, ctx } = params;
			const t = local(animatedTime(params), "animTime");
			const vol = frame.volumetric;
			const EPS = .008;
			const sC = local(frame.surf0, "sC");
			const pxH = local(div(1, ctx.viewportSize.member("y")), "pxH");
			const sdf = local(div(sC.member("x"), u.scale), "sdf");
			const sR = local(frame.surfX, "sR");
			const sL = local(frame.surfL, "sL");
			const sU = local(frame.surfY, "sU");
			const sD = local(frame.surfD, "sD");
			const sharp = max(mul(u.edgeSoftness, .5), .001);
			const gradX = local(div(sub(sR.member("x"), sL.member("x")), EPS * 2), "gradX");
			const thickness = local(max(mul(sdf, -1), 0), "thickness");
			const thin = local(exp(mul(thickness, -5.5)), "thin");
			let rimBase;
			let depthMetric;
			if (vol) {
				const surfFres = pow(clamp(sub(1, abs(local(normalize(vec3(clamp(div(sub(sR.member("w"), sL.member("w")), EPS * 2), -2, 2), clamp(div(sub(sU.member("w"), sD.member("w")), EPS * 2), -2, 2), -1)), "nrm").member("z"))), 0, 1), 2.2);
				rimBase = add(mul(thin, .55), surfFres);
				depthMetric = sC.member("w");
			} else {
				rimBase = thin;
				depthMetric = thickness;
			}
			const rim = local(mul(rimBase, u.edgeGlow), "rim");
			const bodyFill = mul(sub(1, exp(mul(thickness, -5))), u.fill);
			const f = local(fract(sub(mul(depthMetric, u.depthScale), mul(t, 1.2))), "phaseFract");
			const contour = mul(smoothstep(.06, 0, min(f, sub(1, f))), u.depthLines);
			const scan = add(mul(sin(mul(sub(mul(ctx.uv.member("y"), u.scanlineScale), mul(t, mul(u.scanlineSpeed, 3))), TWO_PI)), .5), .5);
			const scanMask = local(mix(float(1), scan, u.scanlines), "scanMask");
			const sweepPos = fract(mul(t, .16));
			const sweepD = local(sub(ctx.uv.member("y"), sweepPos), "sweepD");
			const sweepGlow = local(mul(exp(mul(mul(sweepD, sweepD), -650)), u.sweep), "sweepGlow");
			const shimmer = add(mul(sin(mul(t, 40)), .04), mul(sin(mul(t, 23.3)), .03));
			const drop = smoothstep(.6, .95, add(mul(surfaceNoise(mul(t, 6), 11), .5), .5));
			const flick = mul(add(1, shimmer), sub(1, mul(mul(u.flicker, drop), .7)));
			const grainMul = add(1, mul(mul(sub(hashNoise(add(mul(ctx.uv.member("x"), ctx.viewportSize.member("x")), mul(t, 57.3)), mul(ctx.uv.member("y"), ctx.viewportSize.member("y"))), .5), u.grain), .6));
			const core = local(mul(add(mul(mul(mul(add(add(bodyFill, rim), contour), scanMask), flick), grainMul), mul(sweepGlow, scanMask)), u.brightness), "core");
			const ca = add(mul(u.distortion, .12), .04);
			const fringe = local(mul(mul(mul(rim, gradX), ca), .1), "fringe");
			const rCore = add(core, fringe);
			const bCore = sub(core, fringe);
			const color = u.color.member("rgb");
			const rgb = vec3(mul(color.member("x"), rCore), mul(color.member("y"), core), mul(color.member("z"), bCore));
			const hot = clamp(add(mul(rim, .5), sweepGlow), 0, .8);
			const hotRgb = local(mix(rgb, splat3(core), splat3(hot)), "hotRgb");
			const transition = smoothstep(0, 1, clamp(mul(div(mul(sdf, -1), sharp), 32), 0, 1));
			const alpha = mul(clamp(core, 0, 1), transition);
			return guarded(insideShape(sdf, pxH), vec4(hotRgb, alpha), ZERO, "hologram");
		}
	}),
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
			description: "Center position of the hologram shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the hologram shape (1 = default size)",
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
			description: "Rotation of the hologram shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		color: {
			default: "#5ec8ff",
			transform: transformColor,
			description: "Projection tint — the color the whole hologram emits (classic sci-fi cyan by default)",
			ui: {
				type: "color",
				label: "Color",
				group: "Style"
			}
		},
		brightness: {
			default: 2.5,
			description: "Overall emission gain. Bright edges blow out toward white for a hot, projected look.",
			ui: {
				type: "range",
				min: .2,
				max: 3,
				step: .01,
				label: "Brightness",
				group: "Style"
			}
		},
		edgeSoftness: {
			default: .04,
			description: "Softness of the shape boundary edge — keep low for crisp projection edges",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Softness",
				group: "Style"
			}
		},
		fill: {
			default: .45,
			description: "Translucent interior glow. Driven by optical thickness, so thicker parts of a 3D form glow more — the core volumetric cue.",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Volume Fill",
				group: "Volume"
			}
		},
		edgeGlow: {
			default: 1,
			description: "Fresnel rim intensity — bright glowing silhouette and, on 3D shapes, the grazing internal walls of extruded logos",
			ui: {
				type: "range",
				min: 0,
				max: 2.5,
				step: .01,
				label: "Edge Glow",
				group: "Volume"
			}
		},
		depthLines: {
			default: .5,
			description: "Iso-depth scan-lines that hug the surface and reveal the 3D form like a volumetric scan. On flat shapes they become concentric contour rings.",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Depth Lines",
				group: "Volume"
			}
		},
		depthScale: {
			default: 26,
			description: "Density of the depth scan-lines — higher packs more contour bands through the volume",
			ui: {
				type: "range",
				min: 4,
				max: 70,
				step: 1,
				label: "Depth Density",
				group: "Volume"
			}
		},
		scanlines: {
			default: .55,
			description: "CRT scanline contrast — how dark the gaps between horizontal projection lines get",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Scanlines",
				group: "Scanlines"
			}
		},
		scanlineScale: {
			default: 130,
			description: "Number of horizontal scanlines across the canvas height",
			ui: {
				type: "range",
				min: 20,
				max: 400,
				step: 1,
				label: "Scanline Count",
				group: "Scanlines"
			}
		},
		scanlineSpeed: {
			default: 1,
			description: "How fast the scanlines drift upward — 0 holds them static",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .01,
				label: "Scanline Drift",
				group: "Scanlines"
			}
		},
		sweep: {
			default: .1,
			description: "Brightness of the refresh bar that sweeps vertically through the projection",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Sweep Bar",
				group: "Scanlines"
			}
		},
		flicker: {
			default: .3,
			description: "Unstable-projector flicker — a fast shimmer plus occasional brightness dropouts",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Flicker",
				group: "Interference"
			}
		},
		distortion: {
			default: .1,
			description: "Continuous horizontal wobble and chromatic edge fringing — the wavering of an unstable beam",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Distortion",
				group: "Interference"
			}
		},
		grain: {
			default: .7,
			description: "Fine projection static — animated per-pixel noise over the emission",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Grain",
				group: "Interference"
			}
		},
		speed: {
			default: 1,
			description: "Master animation speed for every animated layer. 0 freezes the projection.",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Speed",
				group: "Animation"
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
			description: "URL to a pre-generated SDF .bin file — when non-empty, activates SVG mode and triggers a shader recompile"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type — triggers recompile when shape is switched. When empty, derived from shape JSON at mount time."
		}
	}
});
var Hologram_default = componentDefinition;
export { componentDefinition as n, Hologram_default as t };
