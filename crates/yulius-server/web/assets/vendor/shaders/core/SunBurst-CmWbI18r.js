import { Ac as defineStd, Ba as softRayLobes, Ma as modulate, Ml as p, Pa as radialFrame, Ua as withFrame, ha as coverageMix, ya as featherMask } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "SunBurst",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Radial sunburst rays emanating from a center point",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		color: {
			default: "#ffdd88",
			transform: transformColor,
			description: "Ray color",
			ui: {
				type: "color",
				label: "Color",
				group: "Colors"
			}
		},
		background: {
			default: "#000000",
			transform: transformColor,
			description: "Background color",
			ui: {
				type: "color",
				label: "Background",
				group: "Colors"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center point of the sunburst",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		rayCount: {
			default: 12,
			description: "Number of rays",
			ui: {
				type: "range",
				min: 3,
				max: 64,
				step: 1,
				label: "Ray Count",
				group: "Effect"
			}
		},
		softness: {
			default: .3,
			description: "Softness of ray edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		radius: {
			default: .8,
			description: "How far the rays extend from the center",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1.2,
				step: .01,
				label: "Radius",
				group: "Effect"
			}
		},
		feather: {
			default: .5,
			description: "How gradually the rays fade at their outer edge",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .01,
				label: "Feather",
				group: "Effect"
			}
		},
		speed: {
			default: .2,
			description: "Rotation speed — positive values rotate clockwise",
			ui: {
				type: "range",
				min: -2,
				max: 2,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		}
	},
	paint: coverageMix({
		color: p("color"),
		background: p("background"),
		coverage: withFrame(radialFrame(p("center")), modulate(softRayLobes({
			count: p("rayCount"),
			softness: p("softness"),
			spin: -1
		}), featherMask({
			radius: p("radius"),
			feather: p("feather")
		})), "burstFrame"),
		hint: "burstAlpha"
	})
});
var SunBurst_default = componentDefinition;
export { componentDefinition as n, SunBurst_default as t };
