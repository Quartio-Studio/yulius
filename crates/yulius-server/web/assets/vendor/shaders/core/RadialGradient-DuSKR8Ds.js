import { Ac as defineStd, Bo as rings, Ml as p, ko as dist, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "RadialGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Radial gradient radiating from a center point",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#ff0000",
			transform: transformColor,
			description: "The starting color at the center of the gradient",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#0000ff",
			transform: transformColor,
			description: "The ending color at the edge of the gradient",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point of the radial gradient",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		radius: {
			default: 1,
			description: "The radius of the gradient (normalized, 0.0-1.0)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		repeat: {
			default: 1,
			description: "Number of times the gradient repeats. Values above 1 create concentric rings.",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 20,
				step: .5,
				label: "Repeat",
				group: "Effect"
			}
		},
		aspect: {
			default: 1,
			description: "Stretches the gradient into an ellipse. Values below 1 compress vertically, above 1 compress horizontally.",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 4,
				step: .01,
				label: "Aspect",
				group: "Effect"
			}
		},
		skewAngle: {
			default: 0,
			description: "Rotates the ellipse axis in degrees. Only visible when Aspect is not 1.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Skew Angle",
				group: "Effect"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: rampOver(rings(dist.radial({
		center: p("center"),
		radius: p("radius"),
		aspect: p("aspect"),
		skew: p("skewAngle")
	}), p("repeat")), stops(p("colorSpace")))
});
var RadialGradient_default = componentDefinition;
export { componentDefinition as n, RadialGradient_default as t };
