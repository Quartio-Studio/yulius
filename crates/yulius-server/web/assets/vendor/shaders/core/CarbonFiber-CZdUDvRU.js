import { $s as fract, Ac as defineStd, Ai as viewRay, Bd as ZERO, Fl as glassShellProps, Gr as geometricNormal, Gs as clamp, Jr as grazingOf, Js as dot, Oc as vec4, Pr as anisoSpecular, Qs as floor, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Si as surfaceNoise, Tc as sub, Ti as tiltAlong, Us as abs, Ws as add, Yr as guarded, Zr as insideShape, Zs as float, ar as direction, bc as smoothstep, bi as studioSoftboxes, dc as mul, hc as reflect, hf as PI$1, hi as shapedSurface, lc as min, mc as pow, ni as lightVec3, oc as local, or as directionFrame, qs as div, tc as ge, uc as mix, vc as select, vi as silhouette, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var PI = PI$1;
var TWO_PI_SEVEN = 2 * Math.PI * 7;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
const componentDefinition = defineStd({
	name: "CarbonFiber",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Photorealistic woven carbon fibre — interlaced tows whose anisotropic sheen flips ninety degrees cell to cell, raised into a quilted weave and finished with a glossy clearcoat that mirrors the studio. Plain or twill, in any shape.",
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
			description: "Center position of the carbon shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the carbon shape (1 = default size)",
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
			description: "Rotation of the carbon shape in degrees",
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
			default: "#b9bdc6",
			transform: transformColor,
			description: "The bright tone of the fibre sheen — recolor for forged/colored carbon (red, blue, bronze)",
			ui: {
				type: "color",
				label: "Fiber Sheen",
				group: "Carbon"
			}
		},
		darkColor: {
			default: "#0b0c0e",
			transform: transformColor,
			description: "The deep resin tone the weave falls to in shadow — near-black for classic carbon",
			ui: {
				type: "color",
				label: "Resin Color",
				group: "Carbon"
			}
		},
		weaveStyle: {
			default: "twill",
			compileTime: true,
			description: "Weave pattern — plain (checkerboard) or 2×2 twill (the diagonal supercar weave)",
			ui: {
				type: "select",
				options: [{
					label: "Twill (2×2)",
					value: "twill"
				}, {
					label: "Plain",
					value: "plain"
				}],
				label: "Weave",
				group: "Carbon"
			}
		},
		weaveScale: {
			default: 30,
			description: "Number of woven tows across the shape — higher = finer, denser weave",
			ui: {
				type: "range",
				min: 2,
				max: 50,
				step: .5,
				label: "Weave Scale",
				group: "Carbon"
			}
		},
		weaveAngle: {
			default: 0,
			description: "Rotates the whole weave grid in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Weave Angle",
				group: "Carbon"
			}
		},
		relief: {
			default: .5,
			description: "Depth of the quilted weave — how far each tow bulges, catching light on its crown and shadowing the valleys between",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Weave Depth",
				group: "Carbon"
			}
		},
		fiberSheen: {
			default: 1.2,
			description: "Strength of the anisotropic fibre sheen — the directional satin glint running along each tow",
			ui: {
				type: "range",
				min: 0,
				max: 1.5,
				step: .01,
				label: "Fiber Sheen",
				group: "Carbon"
			}
		},
		roughness: {
			default: .5,
			description: "Softness of the fibre sheen — 0 = tight crisp glints, 1 = soft matte satin",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Roughness",
				group: "Surface"
			}
		},
		clearcoat: {
			default: 1.2,
			description: "Strength of the glossy lacquer coat — the sharp studio reflections of the wet clearcoat over the weave",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Clearcoat",
				group: "Surface"
			}
		},
		...glassShellProps({
			environment: {
				default: 2,
				description: "Strength of the reflected studio lighting seen in the clearcoat",
				ui: { group: "Surface" }
			},
			envRotation: { ui: { group: "Surface" } },
			lightAngle: {
				default: 215,
				description: "Direction of the key light for the fibre sheen, in degrees",
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
	},
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u, propValues } = params;
			const weaveStyle = (propValues.weaveStyle || "twill") === "plain" ? "plain" : "twill";
			const field = surfaceField(frame, params, { scale: u.scale });
			const { sdf, pxH } = field;
			const nGeo = geometricNormal(frame, field, {
				bevelWidth: u.bevelWidth,
				bevelShape: u.bevelShape
			});
			const weave = directionFrame(u.weaveAngle, "weave");
			const { along: warp, across: weft } = weave.coordsOf(surfacePattern(frame, field));
			const wA = local(mul(warp, u.weaveScale), "wA");
			const wB = local(mul(weft, u.weaveScale), "wB");
			const cellX = local(floor(wA), "cellX");
			const cellY = local(floor(wB), "cellY");
			const fA = local(sub(fract(wA), .5), "fA");
			const fB = local(sub(fract(wB), .5), "fB");
			let parity;
			if (weaveStyle === "plain") {
				const s2 = local(add(cellX, cellY), "s2");
				parity = local(sub(s2, mul(2, floor(div(s2, 2)))), "parity");
			} else {
				const s4 = local(add(sub(cellX, cellY), 64), "s4");
				parity = local(select(ge(sub(s4, mul(4, floor(div(s4, 4)))), 2), float(1), float(0)), "parity");
			}
			const fiber = local(mix(weave.tangent, weave.perp, parity), "fiber");
			const crossAxis = local(mix(weave.perp, weave.tangent, parity), "crossAxis");
			const acrossLocal = local(mix(fB, fA, parity), "acrossLocal");
			const alongLocal = local(mix(warp, weft, parity), "alongLocal");
			const reliefAmt = mul(u.relief, .32);
			const bulge = mul(sin(mul(acrossLocal, PI)), reliefAmt);
			const filJit = mul(surfaceNoise(mul(alongLocal, 2), mul(acrossLocal, 40)), .18);
			const n = tiltAlong(nGeo, crossAxis, local(add(bulge, mul(add(sin(mul(acrossLocal, TWO_PI_SEVEN)), filJit), .13)), "crossTilt"));
			const ao = mix(.5, 1, smoothstep(0, .05, min(sub(.5, abs(fA)), sub(.5, abs(fB)))));
			const viewI = viewRay(params, field);
			const grazing = grazingOf(n, viewI);
			const R = local(reflect(viewI, n), "R");
			const rough = local(clamp(u.roughness, 0, 1), "rough");
			const L = lightVec3(direction(u.lightAngle, "light"), -.7);
			const baseA = local(mix(.06, .5, rough), "baseA");
			const sheen = anisoSpecular({
				normal: n,
				tangent: fiber,
				light: L,
				view: viewI,
				alphas: {
					along: mul(baseA, 6),
					across: baseA
				},
				gain: u.fiberSheen
			});
			const bodyLum = clamp(add(add(.035, mul(clamp(add(mul(dot(n, L), .5), .5), 0, 1), .1)), sheen), 0, 1.3);
			const rgb = add(mul(mix(u.darkColor.member("rgb"), u.lightColor.member("rgb"), splat3(bodyLum)), ao), splat3(local(mul(mul(studioSoftboxes(R.member("x"), R.member("y"), {
				rotation: direction(u.envRotation, "env"),
				keyRadius: .16,
				drift: 0,
				strength: u.environment,
				skyGain: .3
			}), add(.05, mul(pow(grazing, 5), .95))), u.clearcoat), "coat")));
			const transition = silhouette(field, u.edgeSoftness);
			return guarded(insideShape(sdf, pxH), vec4(rgb, transition), ZERO, "carbonFiber");
		}
	})
});
var CarbonFiber_default = componentDefinition;
export { componentDefinition as n, CarbonFiber_default as t };
