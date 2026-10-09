import { $r as keyLightAt, Ac as defineStd, Ai as viewRay, Bd as ZERO, Ec as vec2, Fl as glassShellProps, Gr as geometricNormal, Gs as clamp, Ic as animatedTime, Nu as sdf3d_exports, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Tc as sub, Ur as flowWarp, Ws as add, Yr as guarded, Zr as insideShape, ac as length, ar as direction, bc as smoothstep, dc as mul, gi as sharpGlint, hc as reflect, hi as shapedSurface, li as perlinSlope, oc as local, si as nudgeNormal, uc as mix, vi as silhouette, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var { isMobileGpuViewport } = sdf3d_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
function liquidStudio(rx, ry, envC, envS, keyRad, drift, envStr) {
	const ex = local(sub(mul(rx, envC), mul(ry, envS)), "ex");
	const ey = local(add(mul(rx, envS), mul(ry, envC)), "ey");
	const p = local(vec2(ex, ey), "p");
	const sky = local(smoothstep(-.7, .85, ey), "sky");
	let lum = mul(mul(sky, sky), .4);
	const r0 = local(mul(keyRad, 1.05), "r0");
	const box0 = smoothstep(r0, mul(r0, .2), length(sub(p, vec2(add(0, drift), .55))));
	lum = add(lum, mul(mul(box0, 1.75), envStr));
	const r1 = local(mul(keyRad, 1.25), "r1");
	const box1 = smoothstep(r1, mul(r1, .2), length(sub(p, vec2(add(-.45, drift), -.4))));
	lum = add(lum, mul(mul(box1, 1.15), envStr));
	const r2 = local(mul(keyRad, 1.15), "r2");
	const box2 = smoothstep(r2, mul(r2, .2), length(sub(p, vec2(add(.55, drift), .1))));
	return add(lum, mul(mul(box2, 1.1), envStr));
}
const componentDefinition = defineStd({
	name: "LiquidMetal",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Flowing liquid chrome — a molten reflective surface that wraps the shape, sweeping a procedural studio reflection across animated folds with crisp speculars and prismatic edges",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = animatedTime(params);
			const mobile = isMobileGpuViewport();
			const field = surfaceField(frame, params, { scale: u.scale });
			const { sdf, pxH } = field;
			const nGeo = geometricNormal(frame, field, {
				bevelWidth: u.bevelWidth,
				bevelShape: u.bevelShape
			});
			const pat = surfacePattern(frame, field);
			const freq = local(mul(u.ripple, 2.6), "freq");
			const flowT = local(mul(t, .12), "flowT");
			const q = local(vec2(mul(pat.member("x"), freq), mul(pat.member("y"), freq)), "q");
			const wAmt = mul(u.warp, .9);
			const hg = local(perlinSlope(add(q, mul(flowWarp(mul(q, .6), flowT), wAmt)), .62), "hg");
			const relief = local(mul(u.turbulence, .5), "relief");
			const n = nudgeNormal(nGeo, mul(mul(hg.member("x"), relief), -1), mul(mul(hg.member("y"), relief), -1));
			const R = local(reflect(viewRay(params, field), n), "R");
			const envStr = local(u.environment, "envStr");
			const sharp = local(clamp(u.sharpness, 0, 1), "sharp");
			const keyRad = local(mix(.5, .18, sharp), "keyRad");
			const drift = local(mul(sin(mul(t, .07)), .05), "drift");
			const env = direction(u.envRotation, "env");
			const envC = env.member("x");
			const envS = env.member("y");
			const Hh = local(keyLightAt(direction(u.lightAngle, "light"), -.7).member("H"), "Hh");
			const spec = local(sharpGlint(clamp(add(add(mul(n.member("x"), Hh.member("x")), mul(n.member("y"), Hh.member("y"))), mul(n.member("z"), Hh.member("z"))), 0, 1), sharp, vec4(60, 900, .4, 1.4)), "spec");
			const lumG = local(add(liquidStudio(R.member("x"), R.member("y"), envC, envS, keyRad, drift, envStr), spec), "lumG");
			let lumR;
			let lumB;
			if (mobile) {
				lumR = lumG;
				lumB = lumG;
			} else {
				const grazeD = sub(1, clamp(mul(n.member("z"), -1), 0, 1));
				const dth = local(mul(mul(u.dispersion, .09), add(.4, mul(grazeD, 1.3))), "dth");
				const lumBoffset = local(add(liquidStudio(add(R.member("x"), mul(R.member("y"), dth)), sub(R.member("y"), mul(R.member("x"), dth)), envC, envS, keyRad, drift, envStr), spec), "lumBoffset");
				lumB = lumBoffset;
				lumR = sub(mul(lumG, 2), lumBoffset);
			}
			const dark = local(u.darkColor.member("rgb"), "dark");
			const light = local(u.lightColor.member("rgb"), "light");
			const rgb = vec4(mix(dark.member("x"), light.member("x"), clamp(lumR, 0, 1.8)), mix(dark.member("y"), light.member("y"), clamp(lumG, 0, 1.8)), mix(dark.member("z"), light.member("z"), clamp(lumB, 0, 1.8)), silhouette(field, u.edgeSoftness));
			return guarded(insideShape(sdf, pxH), rgb, ZERO, "liquidMetal");
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
			description: "Center position of the metal shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the metal shape (1 = default size)",
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
			description: "Rotation of the metal shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		lightColor: {
			default: "#eef0f6",
			transform: transformColor,
			description: "The bright tone of the chrome highlights",
			ui: {
				type: "color",
				label: "Light Color",
				group: "Metal"
			}
		},
		darkColor: {
			default: "#141414",
			transform: transformColor,
			description: "The dark tone the reflection falls to in shadow — recolor for tinted metal (gold, steel, copper…)",
			ui: {
				type: "color",
				label: "Dark Color",
				group: "Metal"
			}
		},
		turbulence: {
			default: 1,
			description: "How molten the surface is — the depth of the flowing folds that bend the reflection",
			ui: {
				type: "range",
				min: 0,
				max: 1.5,
				step: .01,
				label: "Turbulence",
				group: "Liquid"
			}
		},
		ripple: {
			default: 4,
			description: "Scale of the molten folds — higher = smaller, busier ripples",
			ui: {
				type: "range",
				min: .5,
				max: 8,
				step: .01,
				label: "Ripple Scale",
				group: "Liquid"
			}
		},
		warp: {
			default: 1.5,
			description: "Swirl of the flow — domain-warps the folds into liquid curls",
			ui: {
				type: "range",
				min: 0,
				max: 1.5,
				step: .01,
				label: "Swirl",
				group: "Liquid"
			}
		},
		speed: {
			default: .5,
			description: "Speed of the molten flow and the slowly drifting reflection. 0 pauses.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .01,
				label: "Speed",
				group: "Liquid"
			}
		},
		...glassShellProps({
			environment: {
				default: 1.5,
				description: "Strength of the reflected studio lighting — the softboxes seen across the metal",
				ui: { group: "Surface" }
			},
			envRotation: { ui: { group: "Surface" } }
		}),
		sharpness: {
			default: .6,
			description: "Tightness of the specular highlights — 1 = razor chrome glints, 0 = soft satin",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Sharpness",
				group: "Surface"
			}
		},
		...glassShellProps({
			dispersion: {
				description: "Prismatic color fringing along the reflection edges",
				ui: { group: "Surface" }
			},
			lightAngle: {
				default: 265,
				description: "Direction of the key light for the specular glints, in degrees",
				ui: { group: "Surface" }
			}
		}),
		bevelWidth: {
			default: .05,
			description: "Width of the edge bevel, relative to the shape — thin for machined jewellery edges, wide for soft pillowed metal",
			ui: {
				type: ["range", "map"],
				min: .005,
				max: .2,
				step: .001,
				label: "Bevel Width",
				group: "Surface"
			}
		},
		bevelShape: {
			default: 0,
			description: "Bevel profile — 0 = one smooth round fillet, 1 = machined: a steep outer fillet, a chamfer plateau and an inner knee, each catching its own line of light",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Bevel Profile",
				group: "Surface"
			}
		},
		...glassShellProps({ edgeSoftness: { ui: { group: "Surface" } } }),
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
	}
});
var LiquidMetal_default = componentDefinition;
export { componentDefinition as n, LiquidMetal_default as t };
