import { Ac as defineStd, Ml as p, mo as strokeOver, uo as ringWaves } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Ripples",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Concentric animated ripples emanating from a point",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point where ripples emanate from",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		colorA: {
			default: "#ffffff",
			transform: transformColor,
			description: "Color of the ripple waves",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#000000",
			transform: transformColor,
			description: "Background color between ripples",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		speed: {
			default: 1,
			description: "Speed of ripple animation",
			ui: {
				type: "range",
				min: -5,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		frequency: {
			default: 20,
			description: "Number of ripples/spacing between them",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 80,
				step: .1,
				label: "Frequency",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Softness of ripple edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .1,
				label: "Softness",
				group: "Effect"
			}
		},
		thickness: {
			default: .5,
			description: "Thickness of each ripple band",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Thickness",
				group: "Effect"
			}
		},
		phase: {
			default: 0,
			description: "Phase offset for ripple animation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 6.28,
				step: .1,
				label: "Phase",
				group: "Animation"
			}
		}
	},
	paint: strokeOver({
		fill: p("colorB"),
		stroke: p("colorA"),
		mask: ringWaves({
			center: p("center"),
			frequency: p("frequency"),
			thickness: p("thickness"),
			softness: p("softness"),
			phase: p("phase")
		})
	})
});
var Ripples_default = componentDefinition;
export { componentDefinition as n, Ripples_default as t };
