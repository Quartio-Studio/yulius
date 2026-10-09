import { Ac as defineStd, Ai as viewRay, Bd as ZERO, Cc as sqrt, Dc as vec3, Ec as vec2, Fl as glassShellProps, Gs as clamp, Hd as call, Ic as animatedTime, Jr as grazingOf, Ks as cos, Nu as sdf3d_exports, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Su as bevel_exports, Tc as sub, Us as abs, Ws as add, Yr as guarded, Ys as exp, Zr as insideShape, Zs as float, ac as length, bc as smoothstep, cc as max, dc as mul, hc as reflect, hi as shapedSurface, lc as min, li as perlinSlope, mf as DEG_TO_RAD$1, oc as local, oi as neutralTone, pf as perlin12d, qs as div, ri as marchedNormal, si as nudgeNormal, uc as mix, vi as silhouette, wd as lighting_exports, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var { bevelSin } = bevel_exports;
var { fieldGradient } = lighting_exports;
var { isMobileGpuViewport } = sdf3d_exports;
var DEG_TO_RAD = DEG_TO_RAD$1;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
function chromeStudio(exIn, eyIn, ezIn, boxSoftIn, stripSoft, spectral, shadows, warmC, coolC) {
	const ex = local(exIn, "ex");
	const ey = local(eyIn, "ey");
	const ez = local(ezIn, "ez");
	const horiz = local(div(ex, add(length(vec2(ex, ey)), 1e-4)), "horiz");
	const warmSide = local(add(.35, mul(.65, smoothstep(-.7, .7, horiz))), "warmSide");
	const coolSide = local(add(.45, mul(.55, smoothstep(-.7, .7, mul(horiz, -1)))), "coolSide");
	const up = local(smoothstep(-.65, 1, ey), "up");
	const graphite = add(mul(coolC, .03), vec3(.003, .003, .004));
	let env = mix(mix(vec3(.17, .175, .19), graphite, splat3(shadows)), vec3(.56, .57, .6), splat3(mul(up, up)));
	const ceil = smoothstep(.3, .9, ey);
	env = add(env, mul(vec3(.55, .55, .56), ceil));
	const cd = local(div(add(ey, .45), .2), "cd");
	const coolBand = exp(mul(mul(cd, cd), -1));
	env = add(env, mul(coolC, mul(mul(mul(coolBand, spectral), .2), coolSide)));
	const facing = local(smoothstep(.18, -.1, ez), "facing");
	const pz = local(max(sub(.32, ez), .06), "pz");
	const px = local(div(ex, pz), "px");
	const py = local(div(ey, pz), "py");
	const bx = local(sub(abs(px), .62), "bx");
	const by = local(sub(abs(sub(py, .3)), .28), "by");
	const inside = min(max(bx, by), 0);
	const dbox = local(add(length(max(vec2(bx, by), vec2(0, 0))), inside), "dbox");
	const boxSoft = local(add(.05, mul(boxSoftIn, .4)), "boxSoft");
	const box = mul(smoothstep(boxSoft, mul(boxSoft, -.6), dbox), facing);
	env = add(env, mul(vec3(2.9, 2.87, 2.82), box));
	const flagMask = smoothstep(.5, .92, abs(ex));
	const flagDepth = mix(.55, .07, shadows);
	env = mul(env, mix(float(1), flagDepth, flagMask));
	const gw = local(add(.065, mul(stripSoft, .05)), "gw");
	const dOut = local(max(dbox, 0), "dOut");
	const warmGlow = mul(exp(mul(mul(div(dOut, gw), div(dOut, gw)), -1)), smoothstep(-.06, .1, dbox));
	env = add(env, mul(warmC, mul(mul(mul(mul(warmGlow, spectral), 1.3), warmSide), facing)));
	const fb = local(div(add(ey, .12), .05), "fb");
	env = add(env, mul(warmC, mul(mul(mul(mul(exp(mul(mul(fb, fb), -1)), spectral), .5), warmSide), sub(1, facing))));
	const cbx = local(sub(abs(px), .7), "cbx");
	const cby = local(sub(abs(add(py, .78)), .05), "cby");
	const dcard = local(add(length(max(vec2(cbx, cby), vec2(0, 0))), min(max(cbx, cby), 0)), "dcard");
	const card = mul(smoothstep(boxSoft, mul(boxSoft, -.6), dcard), facing);
	return add(env, mul(vec3(.5, .5, .52), card));
}
const componentDefinition = defineStd({
	name: "Chrome",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Studio-lit mirror chrome — a precision-machined shape with a controllable polished bevel, softly convex faces and a photographic studio reflected in them: one huge frontal softbox, black flags, and thin warm/cool strip lights that fringe amber and ice-blue exactly where the reflections break",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = animatedTime(params);
			const vol = frame.volumetric;
			const mobile = isMobileGpuViewport();
			const EPS = .01;
			const PR = .0081;
			const field = surfaceField(frame, params, { scale: u.scale });
			const { s0, sX, sY, sdf, pxH } = field;
			const pat = surfacePattern(frame, field);
			const fx = pat.member("x");
			const fy = pat.member("y");
			let nGeo;
			if (vol) nGeo = marchedNormal(field);
			else {
				const grad = local(call(fieldGradient, "fieldGradient", [
					s0,
					sX,
					sY,
					float(EPS)
				]), "grad");
				const glen = local(max(length(grad), 1e-4), "glen");
				const gx = local(div(grad.member("x"), glen), "gx");
				const gy = local(div(grad.member("y"), glen), "gy");
				const bw = max(u.bevelWidth, .003);
				const depth = local(mul(sdf, -1), "depth");
				const t0 = local(clamp(div(depth, bw), 0, 1), "t0");
				const sBev = local(call(bevelSin, "bevelSin", [clamp(add(t0, mul(mul(mul(mul(mul(local(call(perlin12d, "perlin12d", [vec2(mul(fx, 21), mul(fy, 21))]), "rimN").member("x"), u.waviness), .22), t0), sub(1, t0)), 4)), 0, 1), u.bevelShape]), "sBev");
				const sPillow = mul(mul(u.curvature, .14), div(PR, add(mul(depth, depth), PR)));
				const flen = local(length(vec2(fx, fy)), "flen");
				const dgx = local(div(fx, max(flen, 1e-4)), "dgx");
				const dgy = local(div(fy, max(flen, 1e-4)), "dgy");
				const sSphere = local(mul(mul(u.curvature, .75), smoothstep(0, .5, flen)), "sSphere");
				const sEdge = local(max(sBev, sPillow), "sEdge");
				const txy = local(vec2(add(mul(gx, sEdge), mul(dgx, sSphere)), add(mul(gy, sEdge), mul(dgy, sSphere))), "txy");
				const tl = local(min(length(txy), .9995), "tl");
				const tn = local(mul(txy, div(tl, max(length(txy), 1e-4))), "tn");
				const cz = sqrt(sub(1, mul(tl, tl)));
				nGeo = local(vec3(tn.member("x"), tn.member("y"), mul(cz, -1)), "nGeo");
			}
			const wg = local(perlinSlope(vec2(add(mul(fx, 1.05), mul(t, .08)), mul(fy, 1.05)), 1), "wg");
			const wAmt = local(mul(u.waviness, .2), "wAmt");
			const n = nudgeNormal(nGeo, mul(wg.member("x"), wAmt), mul(wg.member("y"), wAmt));
			const viewI = viewRay(params, field, .55);
			const R = local(reflect(viewI, n), "R");
			const tiltV = grazingOf(n, viewI);
			const rotA = local(add(mul(u.envRotation, DEG_TO_RAD), mul(sin(mul(t, .23)), .05)), "rotA");
			const envC = local(cos(rotA), "envC");
			const envS = local(sin(rotA), "envS");
			const upY = local(mul(R.member("y"), -1), "upY");
			const rx0 = local(sub(mul(R.member("x"), envC), mul(upY, envS)), "rx0");
			const ry0 = local(add(mul(R.member("x"), envS), mul(upY, envC)), "ry0");
			const orbit = local(add(mul(sin(mul(t, .5)), .4), mul(sin(add(mul(t, .19), 1.7)), .2)), "orbit");
			const oc = local(cos(orbit), "oc");
			const os = local(sin(orbit), "os");
			const rx = local(add(mul(rx0, oc), mul(R.member("z"), os)), "rx");
			const rz0 = local(sub(mul(R.member("z"), oc), mul(rx0, os)), "rz0");
			const nod = local(mul(sin(add(mul(t, .31), .9)), .12), "nod");
			const nc = local(cos(nod), "nc");
			const ns = local(sin(nod), "ns");
			const ry = local(sub(mul(ry0, nc), mul(rz0, ns)), "ry");
			const rz = local(add(mul(rz0, nc), mul(ry0, ns)), "rz");
			const faceness = smoothstep(.75, 1, mul(n.member("z"), -1));
			const soft = local(u.softness, "soft");
			const softEff = local(add(soft, mul(faceness, .2)), "softEff");
			const spectral = local(u.spectral, "spectral");
			const shadows = local(u.shadows, "shadows");
			const warmC = local(u.warmColor.member("rgb"), "warmC");
			const coolC = local(u.coolColor.member("rgb"), "coolC");
			const envG = local(chromeStudio(rx, ry, rz, softEff, soft, spectral, shadows, warmC, coolC), "envG");
			let envR;
			let envB;
			if (mobile) {
				envR = envG.member("x");
				envB = envG.member("z");
			} else {
				const delta = local(mul(u.dispersion, add(.004, mul(mul(tiltV, tiltV), .06))), "delta");
				const envRt = local(chromeStudio(rx, add(ry, delta), rz, softEff, soft, spectral, shadows, warmC, coolC), "envRt");
				const envBt = local(chromeStudio(rx, sub(ry, delta), rz, softEff, soft, spectral, shadows, warmC, coolC), "envBt");
				envR = envRt.member("x");
				envB = envBt.member("z");
			}
			const rgb = neutralTone(mul(mul(vec3(envR, envG.member("y"), envB), u.environment), u.tint.member("rgb")));
			return guarded(insideShape(sdf, pxH), vec4(rgb, silhouette(field, u.edgeSoftness)), ZERO, "chrome");
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
			description: "Center position of the chrome shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the chrome shape (1 = default size)",
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
			description: "Rotation of the chrome shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		tint: {
			default: "#ffffff",
			transform: transformColor,
			description: "Metal tint multiplied into the reflection — white for silver chrome, warm ivory for gold, dusky rose for copper",
			ui: {
				type: "color",
				label: "Tint",
				group: "Metal"
			}
		},
		warmColor: {
			default: "#ffa94d",
			transform: transformColor,
			description: "Color of the warm strip light hugging the studio horizon — the amber band that fringes the reflections",
			ui: {
				type: "color",
				label: "Warm Light",
				group: "Metal"
			}
		},
		coolColor: {
			default: "#82a7e6",
			transform: transformColor,
			description: "Color of the cool wash below the horizon — the steel-blue zone the darker reflections fall into",
			ui: {
				type: "color",
				label: "Cool Light",
				group: "Metal"
			}
		},
		bevelWidth: {
			default: .028,
			description: "Width of the polished edge bevel, relative to the shape — thin for machined jewellery edges, wide for soft pillowed metal",
			ui: {
				type: ["range", "map"],
				min: .005,
				max: .2,
				step: .001,
				label: "Bevel Width",
				group: "Bevel"
			}
		},
		bevelShape: {
			default: .55,
			description: "Bevel profile — 0 = one smooth round fillet, 1 = machined: a steep outer fillet, a chamfer plateau and an inner knee, each catching its own line of light",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Bevel Profile",
				group: "Bevel"
			}
		},
		curvature: {
			default: .5,
			description: "Convex doming of the faces — sweeps the big soft studio gradients across otherwise flat metal",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Curvature",
				group: "Bevel"
			}
		},
		waviness: {
			default: .15,
			description: "Pressed-metal imperfection — gently bends the face reflections and wobbles the bevel light lines so the surface reads as real",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Waviness",
				group: "Bevel"
			}
		},
		...glassShellProps({
			environment: {
				description: "Exposure of the reflected studio — the overall brilliance of the chrome",
				ui: { group: "Studio" }
			},
			envRotation: {
				description: "Rotates the whole reflected studio — spins where the softbox, flags and strips fall",
				ui: {
					type: ["range", "map"],
					group: "Studio"
				}
			}
		}),
		softness: {
			default: .3,
			description: "Softness of the studio edges — 0 = razor polished reflections, 1 = satin diffusion",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Studio"
			}
		},
		spectral: {
			default: .9,
			description: "The colored accent lights: an amber glow hugging the softbox edge and an ice-blue wash below it. 0 = a fully achromatic black-and-white studio.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Accent Lights",
				group: "Studio"
			}
		},
		...glassShellProps({ dispersion: {
			default: .3,
			description: "Prismatic RGB splitting concentrated on the fast-curving bevel — thin rainbow micro-fringes along the edge highlights, distinct from the broad accent lights",
			ui: { group: "Studio" }
		} }),
		shadows: {
			default: .7,
			description: "Depth of the dark studio reflections — how hard the black side flags and floor crush toward graphite. 0 lifts them to a soft gray studio.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Shadows",
				group: "Studio"
			}
		},
		speed: {
			default: .5,
			description: "Speed of the studio orbit — the whole lighting environment slowly wanders around the piece, sliding the reflections across faces and edges. 0 freezes it.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .01,
				label: "Speed",
				group: "Studio"
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
var Chrome_default = componentDefinition;
export { componentDefinition as n, Chrome_default as t };
