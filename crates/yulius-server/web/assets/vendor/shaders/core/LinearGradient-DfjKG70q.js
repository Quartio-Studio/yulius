import { Ac as defineStd, Ml as p, ko as dist, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, r as transformAngle, s as transformEdges, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "LinearGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Create smooth linear color gradients",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#1aff00",
			transform: transformColor,
			description: "The starting color of the gradient",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#0000ff",
			transform: transformColor,
			description: "The ending color of the gradient",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		start: {
			default: {
				x: 0,
				y: .5
			},
			transform: transformPosition,
			description: "The starting point of the gradient",
			ui: {
				type: "position",
				label: "Start",
				group: "Position"
			}
		},
		end: {
			default: {
				x: 1,
				y: .5
			},
			transform: transformPosition,
			description: "The ending point of the gradient",
			ui: {
				type: "position",
				label: "End",
				group: "Position"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Additional rotation angle of the gradient (in degrees)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		edges: {
			default: "stretch",
			transform: transformEdges,
			compileTime: true,
			description: "How to handle areas beyond the gradient endpoints",
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
					},
					{
						label: "Transparent",
						value: "transparent"
					},
					{
						label: "Mirror",
						value: "mirror"
					},
					{
						label: "Wrap",
						value: "wrap"
					}
				],
				label: "Edges",
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
	paint: rampOver(dist.linear({
		from: p("start"),
		to: p("end"),
		angle: p("angle")
	}), stops(p("colorSpace")), { edges: p("edges") })
});
var LinearGradient_default = componentDefinition;
export { componentDefinition as n, LinearGradient_default as t };
