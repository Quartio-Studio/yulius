import { Aa as lensGhost, Ac as defineStd, Ca as flareGlare, Ea as flareStreak, Ml as p, Sa as flareFrame, Ta as flareStarburst, Vd as asLocal, ba as flareComposite, da as additive, wa as flareHalo, xa as flareCore } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "LensFlare",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Stylize",
	description: "Realistic camera lens flare with artifacts.",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		lightPosition: {
			default: {
				x: .3,
				y: .3
			},
			transform: transformPosition,
			description: "Position of the light source",
			ui: {
				type: "position",
				label: "Light Position",
				group: "Light"
			}
		},
		intensity: {
			default: .5,
			description: "Master brightness of the entire lens flare effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Intensity",
				group: "Light"
			}
		},
		ghostIntensity: {
			default: .4,
			description: "Brightness of internal reflection ghost discs along the flare axis",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Ghosts",
				group: "Reflections"
			}
		},
		ghostSpread: {
			default: .7,
			description: "Spacing between ghost reflections along the flare axis",
			ui: {
				type: "range",
				min: .1,
				max: 2,
				step: .01,
				label: "Ghost Spread",
				group: "Reflections"
			}
		},
		ghostChroma: {
			default: .3,
			description: "Rainbow chromatic fringing around ghost element edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Ghost Chroma",
				group: "Reflections"
			}
		},
		haloIntensity: {
			default: .4,
			description: "Brightness of the circular halo ring from internal reflection",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Halo",
				group: "Reflections"
			}
		},
		haloRadius: {
			default: .6,
			description: "Radius of the halo ring",
			ui: {
				type: "range",
				min: .1,
				max: 1,
				step: .01,
				label: "Halo Size",
				group: "Reflections"
			}
		},
		haloChroma: {
			default: .6,
			description: "Spectral dispersion on the halo creating rainbow color separation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Halo Chroma",
				group: "Reflections"
			}
		},
		haloSoftness: {
			default: .8,
			description: "Thickness and softness of the halo ring",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: 3,
				step: .01,
				label: "Halo Softness",
				group: "Reflections"
			}
		},
		starburstIntensity: {
			default: .3,
			description: "Brightness of diffraction spikes radiating from the light source",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Starburst",
				group: "Effects"
			}
		},
		starburstPoints: {
			default: 6,
			description: "Number of starburst spikes (simulates aperture blade count)",
			ui: {
				type: ["range", "map"],
				min: 4,
				max: 16,
				step: 1,
				label: "Blades",
				group: "Effects"
			}
		},
		streakIntensity: {
			default: .15,
			description: "Brightness of horizontal anamorphic light streak",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Streak",
				group: "Effects"
			}
		},
		streakLength: {
			default: .5,
			description: "Horizontal extent of the anamorphic streak",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 1,
				step: .01,
				label: "Streak Length",
				group: "Effects"
			}
		},
		glareIntensity: {
			default: .2,
			description: "Soft veiling glare that washes out contrast around the light",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Glare",
				group: "Effects"
			}
		},
		glareSize: {
			default: .5,
			description: "Size of the soft glare glow",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 1,
				step: .01,
				label: "Glare Size",
				group: "Effects"
			}
		},
		edgeFade: {
			default: .2,
			description: "How much the flare fades when the light source is near the screen edge (0 = no fade, 1 = heavy fade)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Fade",
				group: "Light"
			}
		},
		speed: {
			default: .5,
			description: "Speed of subtle flare shimmer and starburst rotation",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		}
	},
	paint: (params) => {
		const flare = asLocal(flareFrame({
			position: p("lightPosition"),
			intensity: p("intensity"),
			edgeFade: p("edgeFade")
		})(params), "flare");
		const ghost = lensGhost({
			intensity: p("ghostIntensity"),
			spread: p("ghostSpread"),
			chroma: p("ghostChroma")
		});
		return flareComposite(additive(additive(ghost(flare, [
			.15,
			.2,
			.3,
			.1
		], [
			1,
			.95,
			.9,
			0
		], params), ghost(flare, [
			.3,
			.12,
			.25,
			.65
		], [
			1,
			.85,
			.55,
			2.17
		], params), ghost(flare, [
			.5,
			.08,
			.35,
			.15
		], [
			.62,
			.82,
			1,
			4.34
		], params), ghost(flare, [
			.72,
			.14,
			.18,
			.75
		], [
			.75,
			1,
			.7,
			6.51
		], params), ghost(flare, [
			.95,
			.06,
			.3,
			0
		], [
			1,
			.6,
			.85,
			8.68
		], params), ghost(flare, [
			1.25,
			.1,
			.15,
			.8
		], [
			.55,
			.88,
			1,
			10.85
		], params), ghost(flare, [
			1.55,
			.07,
			.2,
			.25
		], [
			1,
			.92,
			.6,
			13.02
		], params)), flareHalo({
			intensity: p("haloIntensity"),
			radius: p("haloRadius"),
			chroma: p("haloChroma"),
			softness: p("haloSoftness")
		})(flare, params), flareStarburst({
			intensity: p("starburstIntensity"),
			points: p("starburstPoints")
		})(flare, params), flareStreak({
			intensity: p("streakIntensity"),
			length: p("streakLength")
		})(flare, params), flareGlare({
			intensity: p("glareIntensity"),
			size: p("glareSize")
		})(flare, params), flareCore({ intensity: p("intensity") })(flare, params)), flare);
	}
});
var LensFlare_default = componentDefinition;
export { componentDefinition as n, LensFlare_default as t };
