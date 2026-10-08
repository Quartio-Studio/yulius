import { $s as fract, Ac as defineStd, Cc as sqrt, Dc as vec3, Ec as vec2, Fl as glassShellProps, Gs as clamp, Hd as call, Hr as fieldSlope, Ir as cellNoiseAt, Js as dot, Ks as cos, Oc as vec4, Qs as floor, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Tc as sub, Us as abs, Vr as fdSlope, Ws as add, Zs as float, bc as smoothstep, cc as max, dc as mul, ei as keyLightXY, gc as refract, gf as TWO_PI$1, hc as reflect, hi as shapedSurface, lr as sectorFold, mc as pow, mf as DEG_TO_RAD$1, nc as gt, oc as local, pc as normalize, qr as grazingFlat, qs as div, rf as blend_exports, ri as marchedNormal, uc as mix, vc as select, wd as lighting_exports, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, i as transformBoolean } from "./transformations-P50Uo_Ju.js";
var { insideMask } = lighting_exports;
var { unpremultiplyAlpha } = blend_exports;
var DEG_TO_RAD = DEG_TO_RAD$1;
var TWO_PI = TWO_PI$1;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
const componentDefinition = defineStd({
	name: "Crystal",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Diamond-like crystal lens with faceted refraction.",
	requiresRTT: true,
	requiresChild: true,
	...shapedSurface({
		pattern: "triplanar",
		child: "required",
		surface: (frame, params) => {
			const { uniforms: u, propValues, ctx } = params;
			const childTexture = frame.childTexture;
			const vol = frame.volumetric;
			const field = surfaceField(frame, params, { scale: u.scale });
			const suv = local(frame.sdfUV, "suv");
			const aspect = local(div(ctx.viewportSize.member("x"), ctx.viewportSize.member("y")), "aspect");
			const sharp = local(max(mul(u.edgeSoftness, .5), .001), "sharp");
			const { sdf, pxH } = field;
			const cpx = local(u.center.member("x"), "centerPosX");
			const cpy = local(sub(1, u.center.member("y")), "centerPosY");
			const grad = fieldSlope(field);
			const gradX = grad.member("x");
			const gradY = grad.member("y");
			const rb1 = local(call(insideMask, "insideMask", [
				sdf,
				sharp,
				pxH,
				float(1.5)
			]), "rb1");
			const lightRad = mul(u.lightAngle, DEG_TO_RAD);
			const lx = local(add(cos(lightRad), mul(sub(cpx, .5), 3)), "lx");
			const ly = local(add(sin(lightRad), mul(sub(cpy, .5), 3)), "ly");
			const kl1 = keyLightXY(lx, ly, -.75, "kl1");
			const L = kl1.member("L");
			const H = kl1.member("H");
			const kl2 = keyLightXY(mul(ly, -1), lx, -.4, "kl2");
			const L2 = kl2.member("L");
			const H2 = kl2.member("H");
			const depthNorm = clamp(div(mul(sdf, -1), .15), 0, 1);
			const outerWarp = local(mul(sub(1, smoothstep(0, .03, sdf)), .4), "outerWarp");
			const refrScale = mul(u.refraction, .15);
			const haloOffX = local(div(mul(mul(mul(gradX, -1), refrScale), outerWarp), aspect), "haloOffX");
			const haloOffY = local(mul(mul(mul(gradY, -1), refrScale), outerWarp), "haloOffY");
			const pat = surfacePattern(frame, field, { uv: suv });
			const pdx = pat.member("x");
			const pdy = pat.member("y");
			const sAngle = local(div(TWO_PI, u.facets), "sAngle");
			const fUv = local(sectorFold(add(pdx, .5), add(pdy, .5), sAngle), "fUv");
			const crystalRNorm = local(div(sqrt(add(mul(pdx, pdx), mul(pdy, pdy))), .35), "crystalRNorm");
			const tableMask = local(sub(1, smoothstep(.22, .32, crystalRNorm)), "tableMask");
			const radialDensity = add(1, mul(clamp(crystalRNorm, 0, 1), .6));
			const tableSuppress = local(sub(1, mul(tableMask, .9)), "tableSuppress");
			let nSurf;
			if (vol) nSurf = marchedNormal(field, 4, "nSurf");
			else {
				const bevelSlope = local(mul(mul(sub(1, smoothstep(0, 1, depthNorm)), 1.7), sub(1, tableMask)), "bevelSlope");
				nSurf = local(normalize(vec3(mul(gradX, bevelSlope), mul(gradY, bevelSlope), -1)), "nSurf");
			}
			const grazing = grazingFlat(nSurf);
			const feps = .02;
			const vScale1 = local(mul(mul(u.facets, 1.2), radialDensity), "vScale1");
			const fUvS1 = local(mul(fUv, vScale1), "fUvS1");
			const facet1 = fdSlope(cellNoiseAt, fUvS1, feps, "facet1");
			const gx1 = facet1.dx;
			const gy1 = facet1.dy;
			const gLen1 = local(max(sqrt(add(mul(gx1, gx1), mul(gy1, gy1))), .001), "gLen1");
			const nx1 = local(div(gx1, gLen1), "nx1");
			const ny1 = local(div(gy1, gLen1), "ny1");
			const cellId1 = local(floor(fUvS1), "cellId1");
			const cellRand = local(fract(mul(sin(add(mul(cellId1.member("x"), 12.9898), mul(cellId1.member("y"), 78.233))), 43758.5453)), "cellRand");
			const nF1 = local(normalize(add(nSurf, vec3(mul(nx1, .55), mul(ny1, .55), 0))), "nF1");
			const ndlS = local(dot(nSurf, L), "ndlS");
			const ndl1 = local(mul(sub(dot(nF1, L), ndlS), 2.2), "ndl1");
			const facet2 = fdSlope(cellNoiseAt, local(add(mul(fUv, mul(vScale1, 1.7)), vec2(3.7, 1.2)), "fUvS2"), feps, "facet2");
			const gx2 = facet2.dx;
			const gy2 = facet2.dy;
			const gLen2 = local(max(sqrt(add(mul(gx2, gx2), mul(gy2, gy2))), .001), "gLen2");
			const nx2 = local(div(gx2, gLen2), "nx2");
			const ny2 = local(div(gy2, gLen2), "ny2");
			const nF2 = local(normalize(add(nSurf, vec3(mul(nx2, .4), mul(ny2, .4), 0))), "nF2");
			const ndl2 = local(mul(sub(dot(nF2, L2), dot(nSurf, L2)), 2.2), "ndl2");
			const I = vec3(0, 0, 1);
			const facetRefrAmp = local(clamp(mul(u.refraction, .8), 0, 1.6), "facetRefrAmp");
			const refr = local(refract(I, local(normalize(add(add(nSurf, vec3(mul(mul(mul(nx1, .75), facetRefrAmp), tableSuppress), mul(mul(mul(ny1, .75), facetRefrAmp), tableSuppress), 0)), vec3(mul(mul(nx2, .55), facetRefrAmp), mul(mul(ny2, .55), facetRefrAmp), 0))), "nRefr"), div(1, add(1, mul(u.refraction, .55)))), "refr");
			const thickEff = local(vol ? clamp(mul(field.s0.member("x"), -1), 0, .6) : mul(clamp(mul(field.s0.member("x"), -1), 0, .12), 1.6), "thickEff");
			const offMag = local(mul(mul(mul(thickEff, add(.55, mul(cellRand, .9))), u.scale), rb1), "offMag");
			const totalOffX = local(add(div(mul(div(refr.member("x"), max(refr.member("z"), .25)), offMag), aspect), haloOffX), "totalOffX");
			const totalOffY = local(add(mul(div(refr.member("y"), max(refr.member("z"), .25)), offMag), haloOffY), "totalOffY");
			const bounce = local(reflect(refr, local(normalize(vec3(mul(nx2, .7), mul(ny2, .7), -1)), "nBack")), "bounce");
			const bounceMag = local(mul(mul(mul(thickEff, u.scale), rb1), .8), "bounceMag");
			const bounceOffX = div(mul(div(bounce.member("x"), max(abs(bounce.member("z")), .3)), bounceMag), aspect);
			const bounceOffY = mul(div(bounce.member("y"), max(abs(bounce.member("z")), .3)), bounceMag);
			const innerW = local(mul(mul(add(mul(grazing, .5), .18), add(mul(cellRand, .7), .3)), rb1), "innerW");
			const pbx = add(pdx, mul(mul(div(refr.member("x"), max(refr.member("z"), .25)), thickEff), 1.2));
			const pby = add(pdy, mul(mul(div(refr.member("y"), max(refr.member("z"), .25)), thickEff), 1.2));
			const fUvS3 = local(add(mul(sectorFold(add(pbx, .5), add(pby, .5), sAngle), mul(vScale1, 1.4)), vec2(9.1, 4.7)), "fUvS3");
			const facet3 = fdSlope(cellNoiseAt, fUvS3, feps, "facet3");
			const gx3 = facet3.dx;
			const gy3 = facet3.dy;
			const gLen3 = local(max(sqrt(add(mul(gx3, gx3), mul(gy3, gy3))), .001), "gLen3");
			const nx3 = local(div(gx3, gLen3), "nx3");
			const ny3 = local(div(gy3, gLen3), "ny3");
			const cellId3 = local(floor(fUvS3), "cellId3");
			const cellRand3 = local(fract(mul(sin(add(mul(cellId3.member("x"), 31.727), mul(cellId3.member("y"), 57.131))), 43758.5453)), "cellRand3");
			const w3 = local(mul(mul(clamp(mul(thickEff, 2.5), 0, 1), clamp(mul(u.refraction, 1.2), 0, 1)), rb1), "w3");
			const wedgeLit = local(add(mul(add(mul(nx3, lx), mul(ny3, ly)), .5), .5), "wedgeLit");
			const mosaicMul = mul(mul(add(mul(cellRand3, .9), .55), add(mul(wedgeLit, .7), .65)), add(mul(smoothstep(.15, .7, gLen3), .45), .55));
			const mosaicFire = mul(mul(smoothstep(.75, 1, mul(wedgeLit, add(cellRand3, .4))), w3), .8);
			const zoomedX = add(cpx, div(sub(ctx.uv.member("x"), cpx), u.innerZoom));
			const zoomedY = add(cpy, div(sub(ctx.uv.member("y"), cpy), u.innerZoom));
			const lensX = local(add(zoomedX, totalOffX), "lensX");
			const lensY = local(add(zoomedY, totalOffY), "lensY");
			const lit1 = clamp(ndl1, 0, 1);
			const shadow1 = clamp(mul(ndl1, -1), 0, 1);
			const bright1 = max(sub(add(1, mul(lit1, u.highlights)), mul(shadow1, u.shadows)), .02);
			const lit2 = clamp(ndl2, 0, 1);
			const shadow2 = clamp(mul(ndl2, -1), 0, 1);
			const bright2 = sub(add(1, mul(lit2, mul(u.highlights, .5))), mul(shadow2, mul(u.shadows, .5)));
			const surfBright = sub(add(1, mul(mul(clamp(ndlS, 0, 1), u.highlights), .4)), mul(mul(clamp(mul(ndlS, -1), 0, 1), u.shadows), .6));
			const combinedBright = mul(mul(bright1, bright2), surfBright);
			const chrScale = mul(u.dispersion, .12);
			const chrOffX = local(mul(totalOffX, chrScale), "chrOffX");
			const chrOffY = local(mul(totalOffY, chrScale), "chrOffY");
			const edgeDarken = add(mul(mix(smoothstep(.2, .8, gLen1), 1, tableMask), .4), .6);
			const facetMul = mul(mul(mul(add(combinedBright, mul(tableMask, .25)), edgeDarken), mix(float(1), mosaicMul, w3)), u.brightness);
			const ndh1 = local(clamp(dot(nF1, H), 0, 1), "ndh1");
			const ndh2 = clamp(dot(nF2, H2), 0, 1);
			const spec1 = mul(pow(ndh1, 110), 2.2);
			const spec2 = mul(pow(ndh2, 56), .9);
			const glint = mul(smoothstep(.965, .995, ndh1), 3);
			const flashContrib = local(mul(mul(add(add(add(spec1, spec2), glint), mosaicFire), u.highlights), u.brightness), "flashContrib");
			const fresnelRim = mul(mul(mul(pow(grazing, mix(float(6), float(1.5), clamp(mul(u.fresnelSoftness, .5), 0, 1))), u.fresnel), 2), rb1);
			const fresnelContrib = mul(u.fresnelColor.member("rgb"), fresnelRim);
			const lightFacing = add(mul(add(mul(gradX, lx), mul(gradY, ly)), .5), .5);
			const rb2 = local(mul(mul(sub(clamp(mul(div(mul(sub(sdf, pxH), -1), sharp), 16), 0, 1), clamp(mul(div(mul(sdf, -1), sharp), 16), 0, 1)), lightFacing), .35), "rb2");
			const transition = local(smoothstep(0, 1, rb1), "transition");
			const cutoutOn = Number(propValues.cutout) > 0;
			const outerUV = vec2(add(ctx.uv.member("x"), haloOffX), add(ctx.uv.member("y"), haloOffY));
			const baseSampleUV = cutoutOn ? ctx.uv : outerUV;
			const sampledR = local(call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(vec2(add(lensX, chrOffX), add(lensY, chrOffY)))]), "sampledR");
			const sampledG = local(call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(vec2(lensX, lensY))]), "sampledG");
			const sampledB = local(call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(vec2(sub(lensX, chrOffX), sub(lensY, chrOffY)))]), "sampledB");
			const innerSample = local(call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(vec2(add(lensX, bounceOffX), add(lensY, bounceOffY)))]), "innerSample");
			const baseColor = local(call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(baseSampleUV)]), "baseColor");
			const refractedRgb = mix(vec3(sampledR.member("x"), sampledG.member("y"), sampledB.member("z")), vec3(innerSample.member("x"), innerSample.member("y"), innerSample.member("z")), splat3(innerW));
			const refractedA = sampledG.member("w");
			const litRgb = local(mul(refractedRgb, facetMul), "litRgb");
			const lumWeights = vec3(.299, .587, .114);
			const origLum = dot(litRgb, lumWeights);
			const tinted = local(mix(litRgb, u.tintColor.member("rgb"), splat3(u.tintIntensity)), "tinted");
			const lumPreserved = mul(tinted, div(origLum, max(dot(tinted, lumWeights), 1e-4)));
			const composited = local(mix(baseColor, vec4(add(add(add(select(gt(u.tintPreserveLuminosity, .5), lumPreserved, tinted), splat3(flashContrib)), fresnelContrib), splat3(rb2)), refractedA), vec4(transition)), "composited");
			const cutoutAlpha = cutoutOn ? transition : float(1);
			return vec4(composited.member("xyz"), mul(composited.member("w"), cutoutAlpha));
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
			description: "Center position of the crystal shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the crystal shape (1 = default size)",
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
			description: "Rotation of the crystal shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		cutout: {
			default: false,
			transform: transformBoolean,
			description: "Cut out alpha outside the crystal shape",
			compileTime: true,
			ui: {
				type: "checkbox",
				label: "Cutout",
				group: "Crystal"
			}
		},
		...glassShellProps({
			refraction: {
				description: "How strongly the crystal refracts content beneath",
				ui: {
					type: "range",
					max: 3,
					group: "Crystal"
				}
			},
			dispersion: {
				default: .5,
				description: "Prismatic rainbow dispersion — splits light into spectral colors",
				ui: {
					type: "range",
					max: 2,
					group: "Crystal"
				}
			}
		}),
		facets: {
			default: 5,
			description: "Symmetry order — how many times the facet pattern repeats around the center",
			ui: {
				type: "range",
				min: 3,
				max: 24,
				step: 1,
				label: "Facets",
				group: "Crystal"
			}
		},
		...glassShellProps({
			fresnel: {
				default: .05,
				description: "Fresnel rim glow intensity around the crystal boundary",
				ui: { type: "range" }
			},
			fresnelSoftness: {
				default: 1,
				ui: {
					type: "range",
					max: 2
				}
			},
			fresnelColor: true,
			edgeSoftness: {
				default: 0,
				description: "Softness of the crystal boundary edge",
				ui: {
					type: "range",
					group: "Crystal"
				}
			}
		}),
		innerZoom: {
			default: 1.5,
			description: "Magnification of content seen through the crystal",
			ui: {
				type: "range",
				min: .5,
				max: 3,
				step: .01,
				label: "Inner Zoom",
				group: "Crystal"
			}
		},
		...glassShellProps({ lightAngle: {
			default: 270,
			description: "Light direction angle in degrees",
			ui: { group: "Lighting" }
		} }),
		highlights: {
			default: .5,
			description: "Additive brightness on light-facing facets — never darkens",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Highlights",
				group: "Lighting"
			}
		},
		shadows: {
			default: .3,
			description: "Darkening on shadow-facing facets — never brightens",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Shadows",
				group: "Lighting"
			}
		},
		brightness: {
			default: 1.2,
			description: "Overall crystal brightness — higher values push facets toward brilliant white",
			ui: {
				type: "range",
				min: .5,
				max: 3,
				step: .01,
				label: "Brightness",
				group: "Lighting"
			}
		},
		tintColor: {
			default: "#e8e0ff",
			transform: transformColor,
			description: "Crystal body tint color",
			ui: {
				type: "color",
				label: "Tint Color",
				group: "Tint"
			}
		},
		tintIntensity: {
			default: 0,
			description: "How much tint color is applied to the crystal interior",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Tint Intensity",
				group: "Tint"
			}
		},
		tintPreserveLuminosity: {
			default: true,
			transform: transformBoolean,
			description: "Preserve original brightness when tinting",
			ui: {
				type: "checkbox",
				label: "Preserve Luminosity",
				group: "Tint"
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
var Crystal_default = componentDefinition;
export { componentDefinition as n, Crystal_default as t };
