import { $r as keyLightAt, Ac as defineStd, Ai as viewRay, Ar as parallaxPlane, Bd as ZERO, Ci as surfaceNoiseAt, Dc as vec3, Ec as vec2, Fl as glassShellProps, Gr as geometricNormal, Gs as clamp, Hd as call, Ic as animatedTime, Js as dot, Mr as volumeMarch, Nu as sdf3d_exports, Oc as vec4, Oi as twinkle, Or as layers, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Tc as sub, Uf as vec3f, Us as abs, Ws as add, Xr as hashNoise, Yr as guarded, Zr as insideShape, Zs as float, _i as shellRefract, ar as direction, bc as smoothstep, bi as studioSoftboxes, dc as mul, df as mixPreconvertedVariants, fi as pointStars, hc as reflect, hi as shapedSurface, jr as turbulentMedium, kr as interiorRay, lf as mixPreconvertedInSpace, mc as pow, oc as local, of as convertP3ToMixSpaceCPU, oi as neutralTone, pi as rotationSensor, rd as sdf_exports, sf as mixColorsLinear, uc as mix, uf as mixPreconvertedLinear, vi as silhouette, xi as surfaceField, zr as dualLobeGlint } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var { isMobileGpuViewport, VOLUMETRIC_FIELD_EXTRA_FIELDS } = sdf3d_exports;
var { ANALYTIC_SDF_EXTRA_FIELDS } = sdf_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var NEBULA_STEPS_DESKTOP = 6;
var NEBULA_STEPS_MOBILE = 4;
const componentDefinition = defineStd({
	name: "Nebula",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "A volumetric gas nebula sealed inside polished glass; billowing emission clouds with hollow dark cavities and hot glowing cores, star fields drifting at real depth behind the gas, all refracted through the curved walls of the shape and dressed with studio reflections",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = animatedTime(params);
			const steps = isMobileGpuViewport() ? NEBULA_STEPS_MOBILE : NEBULA_STEPS_DESKTOP;
			const spaceMode = params.propValues.colorSpace ?? 0;
			let veilGasMix;
			let rampMix;
			if (spaceMode !== 0) {
				let lastKey = "";
				params.onBeforeRender(() => {
					const cols = [
						"veilColor",
						"gasColor",
						"coreColor"
					].map((name) => params.getCpuValue(name));
					if (cols.some((c) => !c)) return;
					const key = cols.map((c) => `${c.x},${c.y},${c.z}`).join("|");
					if (key === lastKey) return;
					lastKey = key;
					const fields = [
						"convVeil",
						"convGas",
						"convCore"
					];
					cols.forEach((c, idx) => {
						const conv = convertP3ToMixSpaceCPU(c.x, c.y, c.z, spaceMode);
						params.setExtraField(fields[idx], [
							conv[0],
							conv[1],
							conv[2]
						]);
					});
				});
				const back = mixPreconvertedVariants[spaceMode] ?? mixPreconvertedLinear;
				veilGasMix = (tv) => call(back, "mixPreconvertedColors", [
					u.convVeil,
					u.convGas,
					u.veilColor.member("a"),
					u.gasColor.member("a"),
					tv
				]);
				rampMix = (t1, t2, hint) => {
					const ab = local(call(mixPreconvertedInSpace, "mixInSpace", [
						u.convVeil,
						u.veilColor.member("a"),
						u.convGas,
						u.gasColor.member("a"),
						t1
					]), `${hint}AB`);
					return call(back, "mixPreconvertedColors", [
						ab.member("conv"),
						u.convCore,
						ab.member("alpha"),
						u.coreColor.member("a"),
						t2
					]);
				};
			} else {
				veilGasMix = (tv) => call(mixColorsLinear, "mixColors", [
					u.veilColor,
					u.gasColor,
					tv
				]);
				rampMix = (t1, t2) => call(mixColorsLinear, "mixColors", [
					call(mixColorsLinear, "mixColors", [
						u.veilColor,
						u.gasColor,
						t1
					]),
					u.coreColor,
					t2
				]);
			}
			const field = surfaceField(frame, params, { scale: u.scale });
			const { sdf, pxH } = field;
			const n = geometricNormal(frame, field, {
				bevelWidth: float(.09),
				bevelShape: float(0)
			});
			const view = viewRay(params, field);
			const interior = shellRefract(frame, n, local(mul(u.refraction, .16), "bend"));
			const { x: sx, y: sy } = interior.centered;
			const sensor = rotationSensor(frame);
			const ray = interiorRay(field, frame, {
				origin: interior,
				view,
				shear: .55,
				lengthScale: .9
			});
			const pathLen = ray.length;
			const freq = local(mul(u.gasScale, 3.1), "freq");
			const drift = local(mul(t, .14), "drift");
			const seedOff = local(mul(u.seed, 7.31), "seedOff");
			const jit = local(mul(hashNoise(mul(interior.x, 311.7), mul(interior.y, 173.3)), .15 / steps), "jit");
			const macro = local(mul(surfaceNoiseAt(vec2(add(add(mul(sx, 1.3), mul(drift, .6)), mul(seedOff, .37)), add(mul(sy, 1.3), mul(seedOff, .61)))), .5), "macro");
			const cav = local(mix(-.35, .5, u.cavity), "cav");
			const billowAmt = local(mul(u.billow, .55), "billowAmt");
			const core = local(u.coreColor.member("rgb"), "coreRgb");
			const veil = local(u.veilColor.member("rgb"), "veilRgb");
			const eGain = local(mul(add(.3, mul(pathLen, .9)), 6 / steps), "eGain");
			const aGain = local(mul(mul(add(.5, mul(pathLen, 2.2)), u.density), 3 / steps), "aGain");
			const march = volumeMarch({
				steps,
				entry: ray.entry,
				length: ray.length,
				jitter: jit,
				emissionGain: eGain,
				hint: "gasAcc"
			}, ({ i, t: fi, z }) => {
				const pos = ray.at(z);
				const gasN = turbulentMedium({
					x: pos.x,
					y: pos.y,
					z
				}, {
					frequency: freq,
					drift,
					seed: seedOff,
					billow: billowAmt,
					hint: `${i}`
				});
				const wall = smoothstep(0, .11, mul(pathLen, Math.min(fi, 1 - fi)));
				const ridge = sub(1, clamp(mul(abs(gasN.fine), 1.6), 0, 1));
				const dens = local(mul(mul(smoothstep(0, .5, sub(add(gasN.density, macro), cav)), wall), add(.65, mul(ridge, .7))), `dens${i}`);
				const dustD = local(mul(mul(smoothstep(.45, .75, gasN.mid), wall), u.dust), `dust${i}`);
				const cB = local(rampMix(smoothstep(.05, .75, dens), smoothstep(.68, 1, dens), `cB${i}`), `cB${i}`).member("rgb");
				const dim = 1 - fi * .45;
				const haze = mul(smoothstep(-.7, .3, gasN.density), mul(.05, wall));
				return {
					emit: add(add(mul(cB, mul(dens, dim)), mul(core, mul(pow(dens, 3), mul(u.glow, 1.2 * dim)))), mul(veil, mul(haze, dim))),
					absorb: add(splat3(mul(dens, aGain)), mul(vec3(1.5, 1, .55), mul(dustD, mul(aGain, 2.2))))
				};
			});
			const starFreq = local(mul(u.starScale, 34), "starFreq");
			const twinkleAmt = local(mul(u.twinkle, .5), "twinkleAmt");
			const farPlane = parallaxPlane(ray, {
				depth: 1.1,
				pan: sensor,
				panRate: 1.3,
				hint: "far"
			});
			const far = pointStars(local(vec2(mul(farPlane.x, starFreq), mul(farPlane.y, starFreq)), "farP"), {
				variationFreq: .13,
				radius: [.004, .013],
				keep: [.35, .75],
				brightness: [.3, 1.2],
				shimmer: (bv) => twinkle(t, bv, {
					amount: twinkleAmt,
					rate: 1.7,
					spread: 40
				}),
				gain: u.stars,
				tint: {
					from: [
						.72,
						.82,
						1
					],
					to: [
						1,
						.92,
						.82
					],
					gain: 1.6
				},
				hint: "starFar"
			});
			const nearPlane = parallaxPlane(ray, {
				depth: .25,
				pan: sensor,
				panRate: .55,
				hint: "near"
			});
			const nearFreq = mul(starFreq, .56);
			const near = pointStars(local(vec2(add(mul(nearPlane.x, nearFreq), 7.3), add(mul(nearPlane.y, nearFreq), 2.9)), "nearP"), {
				variationFreq: .11,
				radius: [.003, .01],
				keep: [.45, .8],
				brightness: [.35, 1.3],
				shimmer: (bv) => twinkle(t, bv, {
					amount: twinkleAmt,
					rate: 2.3,
					spread: 34
				}),
				gain: u.stars,
				tint: {
					from: [
						.75,
						.85,
						1
					],
					to: [
						1,
						.95,
						.88
					],
					gain: 1.8
				},
				hint: "starNear"
			});
			const lightDir = direction(u.lightAngle, "light");
			const ndh = local(clamp(dot(n, keyLightAt(lightDir, -.7).member("H")), 0, 1), "ndh");
			const R = local(reflect(view, n), "R");
			const envLum = local(studioSoftboxes(R.member("x"), R.member("y"), {
				rotation: lightDir,
				keyRadius: .2,
				drift: 0,
				strength: u.environment,
				skyGain: .32
			}), "envLum");
			const spec = dualLobeGlint(ndh, {
				core: [140, 1.3],
				halo: [22, .14],
				softness: u.highlightSoftness,
				gain: u.highlight
			});
			const rgb = neutralTone(layers([
				{
					paint: veilGasMix(float(.25)).member("rgb"),
					opacity: .06,
					behind: march.transmittance
				},
				march.color,
				{
					paint: far.rgb,
					behind: march.transmittance
				},
				{
					paint: near.rgb,
					behind: mix(march.transmittance, splat3(1), .35)
				},
				{
					paint: mul(vec3(.85, .92, 1.08), envLum),
					opacity: .38
				},
				mul(vec3(1, .98, .95), spec)
			]));
			return guarded(insideShape(sdf, pxH), vec4(rgb, silhouette(field, u.edgeSoftness)), ZERO, "nebula");
		}
	}),
	extraFields: {
		...VOLUMETRIC_FIELD_EXTRA_FIELDS,
		...ANALYTIC_SDF_EXTRA_FIELDS,
		convVeil: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		},
		convGas: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		},
		convCore: {
			schema: vec3f,
			initial: [
				0,
				0,
				0
			]
		}
	},
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
			description: "Center position of the nebula shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the nebula shape (1 = default size)",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 3,
				step: .01,
				label: "Scale",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the nebula shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		coreColor: {
			default: "#ffd9b0",
			transform: transformColor,
			description: "The hot color the densest gas cores glow with — the embedded newborn stars",
			ui: {
				type: "color",
				label: "Core Color",
				group: "Nebula"
			}
		},
		gasColor: {
			default: "#ff5e7a",
			transform: transformColor,
			description: "The main emission color of the gas body — H-alpha rose by default",
			ui: {
				type: "color",
				label: "Gas Color",
				group: "Nebula"
			}
		},
		veilColor: {
			default: "#3a5bdb",
			transform: transformColor,
			description: "The color of the thin outer veils and the deep-space background behind the gas",
			ui: {
				type: "color",
				label: "Veil Color",
				group: "Nebula"
			}
		},
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space the veil → gas → core ramps blend in",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Nebula"
			}
		},
		density: {
			default: 1,
			description: "Optical density of the gas — how strongly clouds occlude the stars and gas behind them",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Density",
				group: "Nebula"
			}
		},
		cavity: {
			default: 0,
			description: "How hollowed-out the nebula is — 0 fills the volume with cloud banks, high carves large dark voids between them",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Cavity",
				group: "Nebula"
			}
		},
		dust: {
			default: .55,
			description: "Cold dark dust lanes threading the gas — absorb-only tendrils that redden the clouds and stars behind them",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Dust",
				group: "Nebula"
			}
		},
		gasScale: {
			default: .4,
			description: "Feature size of the gas — higher = finer, busier cloud detail",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 1,
				step: .01,
				label: "Gas Scale",
				group: "Nebula"
			}
		},
		billow: {
			default: .6,
			description: "Turbulent folding of the clouds — 0 = smooth drifting banks, 1 = heavily churned billows",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Billow",
				group: "Nebula"
			}
		},
		glow: {
			default: 0,
			description: "Hot emission bloom of the dense cores",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Glow",
				group: "Nebula"
			}
		},
		seed: {
			default: 0,
			description: "Random seed — pans the gas field to a different region of noise space for a new cloud composition",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Nebula"
			}
		},
		stars: {
			default: .07,
			description: "Brightness of the star fields drifting at depth behind and inside the gas",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Stars",
				group: "Stars"
			}
		},
		starScale: {
			default: 1,
			description: "Density of the star fields — higher = smaller, busier stars",
			ui: {
				type: ["range", "map"],
				min: .25,
				max: 3,
				step: .01,
				label: "Star Scale",
				group: "Stars"
			}
		},
		twinkle: {
			default: .35,
			description: "Intensity of the star twinkle (0 = steady stars, 1 = full shimmer)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Twinkle",
				group: "Stars"
			}
		},
		...glassShellProps({
			refraction: { description: "How strongly the curved glass walls bend the interior seen near the edges" },
			environment: {
				default: .7,
				description: "Strength of the studio softboxes reflected in the polished glass surface"
			},
			highlight: {
				default: 1.15,
				description: "Sharp key-light glint on the glass",
				ui: { group: "Glass" }
			},
			highlightSoftness: { ui: { group: "Glass" } },
			lightAngle: { description: "Direction of the key light and reflected studio, in degrees" },
			edgeSoftness: true
		}),
		speed: {
			default: .3,
			description: "Speed of the slowly churning gas and twinkling stars. 0 freezes the nebula.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
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
			description: "URL to a pre-generated SDF .bin file"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type"
		}
	}
});
var Nebula_default = componentDefinition;
export { componentDefinition as n, Nebula_default as t };
