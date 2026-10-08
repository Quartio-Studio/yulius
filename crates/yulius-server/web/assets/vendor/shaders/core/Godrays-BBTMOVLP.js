import { Ac as defineStd, La as seamlessRadialFrame, Ml as p, Na as noiseRays, Ua as withFrame, ga as coverageOver } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Godrays",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Volumetric light rays emanating from a point",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		center: {
			default: {
				x: 0,
				y: 0
			},
			transform: transformPosition,
			description: "The center point of the god rays",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		density: {
			default: .3,
			description: "Frequency of ray sectors",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Density",
				group: "Effect"
			}
		},
		intensity: {
			default: .8,
			description: "Ray visibility within sectors",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		spotty: {
			default: 1,
			description: "Density of spots on rays (higher = more spots)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Spotty",
				group: "Effect"
			}
		},
		speed: {
			default: .5,
			description: "Animation speed of the rays",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		rayColor: {
			default: "#4283fb",
			transform: transformColor,
			description: "Color of the light rays",
			ui: {
				type: "color",
				label: "Ray Color",
				group: "Colors"
			}
		},
		backgroundColor: {
			default: "transparent",
			transform: transformColor,
			description: "Background color",
			ui: {
				type: "color",
				label: "Background Color",
				group: "Colors"
			}
		}
	},
	paint: coverageOver({
		color: p("rayColor"),
		background: p("backgroundColor"),
		coverage: withFrame(seamlessRadialFrame(p("center")), noiseRays({
			density: p("density"),
			intensity: p("intensity"),
			spotty: p("spotty")
		}))
	})
});
var Godrays_default = componentDefinition;
export { componentDefinition as n, Godrays_default as t };
