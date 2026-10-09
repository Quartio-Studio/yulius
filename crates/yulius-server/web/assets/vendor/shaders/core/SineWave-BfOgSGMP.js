import { Ac as defineStd, Ml as p, Sr as sineStroke } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "SineWave",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Animated wave with thickness and softness",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		color: {
			default: "#ffffff",
			transform: transformColor,
			description: "The color of the sine wave",
			ui: {
				type: "color",
				label: "Color",
				group: "Colors"
			}
		},
		amplitude: {
			default: .15,
			description: "The height/amplitude of the sine wave",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Amplitude",
				group: "Effect"
			}
		},
		frequency: {
			default: 1,
			description: "The frequency/number of wave cycles",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 20,
				step: .1,
				label: "Frequency",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "The animation speed of the wave",
			ui: {
				type: "range",
				min: -5,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "The rotation angle of the wave (in degrees)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		position: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center position of the wave",
			ui: {
				type: "position",
				label: "Position",
				group: "Position"
			}
		},
		thickness: {
			default: .2,
			description: "The thickness of the wave line",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "Thickness",
				group: "Effect"
			}
		},
		softness: {
			default: .4,
			description: "Edge softness of the wave line",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Softness",
				group: "Effect"
			}
		}
	},
	paint: sineStroke({
		position: p("position"),
		angle: p("angle"),
		frequency: p("frequency"),
		amplitude: p("amplitude"),
		thickness: p("thickness"),
		softness: p("softness"),
		color: p("color")
	})
});
var SineWave_default = componentDefinition;
export { componentDefinition as n, SineWave_default as t };
