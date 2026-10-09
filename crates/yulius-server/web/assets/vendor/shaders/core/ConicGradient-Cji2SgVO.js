import { Ac as defineStd, Ml as p, Yo as tiles, ko as dist, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "ConicGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Colors sweep in a full circle around a center point, like a color wheel",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#FF0080",
			transform: transformColor,
			description: "Starting color of the sweep",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#00BFFF",
			transform: transformColor,
			description: "Ending color of the sweep (wraps back to Color A)",
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
			description: "Center point of the sweep",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation offset in degrees — shifts where Color A begins",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		},
		repeat: {
			default: 1,
			description: "Number of times the gradient repeats around the circle. Values above 1 create a starburst pattern.",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 24,
				step: 1,
				label: "Repeat",
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
	paint: rampOver(tiles(dist.conic({
		center: p("center"),
		rotation: p("rotation")
	}), p("repeat")), stops(p("colorSpace")))
});
var ConicGradient_default = componentDefinition;
export { componentDefinition as n, ConicGradient_default as t };
